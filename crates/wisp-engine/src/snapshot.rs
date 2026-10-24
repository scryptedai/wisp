//! Metadata-only views of history: ancestry, merge skips, and which version of a section a
//! snapshot (a `ChangesetId`) shows. Nothing here reads or decrypts a payload, so it works for
//! erased sections and erased actors exactly as for live ones.

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wisp_model::*;
use wisp_store::Ledger;

/// Ops that create a new version of a section (the version a reader would see afterwards).
pub fn is_section_version(op: Op) -> bool {
    matches!(
        op,
        Op::AddSection
            | Op::UpdateSection
            | Op::AppendSection
            | Op::PrependSection
            | Op::Split
            | Op::MoveSection
    )
}

/// Ops that write section content (used by the missing-inputs warning).
pub fn writes_section_content(op: Op) -> bool {
    matches!(
        op,
        Op::AddSection | Op::UpdateSection | Op::AppendSection | Op::PrependSection | Op::Split
    )
}

/// Ops that create a new version of a concept's frontmatter, treated as one unit.
pub fn is_frontmatter_version(op: Op) -> bool {
    matches!(
        op,
        Op::CreateConcept
            | Op::SetKey
            | Op::UnsetKey
            | Op::SourcePut
            | Op::SourceRemove
            | Op::SetStatus
    )
}

/// The unit a version belongs to: one section, or one concept's frontmatter as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Section(SectionId),
    Frontmatter(ConceptId),
}

impl Unit {
    pub fn kind(&self) -> &'static str {
        match self {
            Unit::Section(_) => "section",
            Unit::Frontmatter(_) => "frontmatter",
        }
    }
}

/// The unit whose state `c` sets (a removal ends a section).
pub fn unit_of(c: &Change) -> Option<Unit> {
    if is_section_version(c.op) || c.op == Op::RemoveSection {
        return c.target.section().map(Unit::Section);
    }
    if is_frontmatter_version(c.op) {
        return c.target.concept().map(Unit::Frontmatter);
    }
    None
}

/// Memoizing reader over the ledger's changeset DAG.
pub struct History<'a> {
    conn: &'a Connection,
    closures: HashMap<ChangesetId, Rc<HashSet<ChangesetId>>>,
    /// changeset → (seq, parents and `based_on`), loaded lazily.
    nodes: HashMap<ChangesetId, Option<(i64, Vec<ChangesetId>)>>,
    ancestry: HashMap<(ChangesetId, ChangesetId), bool>,
    heads_memo: HashMap<(Unit, ChangesetId, Option<u32>), Rc<Vec<Change>>>,
    unit_rows: HashMap<Unit, Rc<Vec<(i64, Change)>>>,
    /// change → merge changesets whose resolution skipped it (any ref).
    skips: HashMap<ChangeId, Vec<ChangesetId>>,
}

impl<'a> History<'a> {
    pub fn new(conn: &'a Connection) -> Result<Self> {
        let mut skips: HashMap<ChangeId, Vec<ChangesetId>> = HashMap::new();
        for (merge_cs, change) in Ledger::all_merge_replay_skips(conn)? {
            skips.entry(change).or_default().push(merge_cs);
        }
        Ok(History {
            conn,
            closures: HashMap::new(),
            nodes: HashMap::new(),
            ancestry: HashMap::new(),
            heads_memo: HashMap::new(),
            unit_rows: HashMap::new(),
            skips,
        })
    }

    pub fn conn(&self) -> &'a Connection {
        self.conn
    }

    /// Whether the changeset exists in the ledger.
    pub fn changeset_exists(&mut self, cs: ChangesetId) -> Result<bool> {
        Ok(self.seq(cs)?.is_some())
    }

    fn node(&mut self, cs: ChangesetId) -> Result<Option<(i64, Vec<ChangesetId>)>> {
        if let Some(n) = self.nodes.get(&cs) {
            return Ok(n.clone());
        }
        let n = Ledger::get_changeset(self.conn, cs)?.map(|row| {
            let mut up = row.changeset.parents.clone();
            if let Some(b) = row.changeset.based_on {
                up.push(b);
            }
            (row.seq, up)
        });
        self.nodes.insert(cs, n.clone());
        Ok(n)
    }

    pub fn seq(&mut self, cs: ChangesetId) -> Result<Option<i64>> {
        Ok(self.node(cs)?.map(|(s, _)| s))
    }

    /// Every changeset reachable from `head` (itself included), following parents and `based_on`.
    pub fn closure(&mut self, head: ChangesetId) -> Result<Rc<HashSet<ChangesetId>>> {
        if let Some(c) = self.closures.get(&head) {
            return Ok(c.clone());
        }
        let mut set = HashSet::new();
        let mut stack = vec![head];
        while let Some(id) = stack.pop() {
            if !set.insert(id) {
                continue;
            }
            match self.node(id)? {
                Some((_, up)) => stack.extend(up),
                None => {
                    set.remove(&id);
                }
            }
        }
        let rc = Rc::new(set);
        self.closures.insert(head, rc.clone());
        Ok(rc)
    }

    /// `a` is `b` or one of its ancestors. A depth-first search from `b` that never descends
    /// below `a`'s seq: ancestors are always inserted before their descendants (commit and sync
    /// both require parents to exist first), so nothing with a lower seq can lead back to `a`.
    pub fn is_ancestor_or_self(&mut self, a: ChangesetId, b: ChangesetId) -> Result<bool> {
        if a == b {
            return Ok(true);
        }
        if let Some(c) = self.closures.get(&b) {
            return Ok(c.contains(&a));
        }
        if let Some(r) = self.ancestry.get(&(a, b)) {
            return Ok(*r);
        }
        let (Some(sa), Some(sb)) = (self.seq(a)?, self.seq(b)?) else {
            return Ok(false);
        };
        let mut found = false;
        if sa <= sb {
            let mut seen = HashSet::new();
            let mut stack = vec![b];
            while let Some(id) = stack.pop() {
                if id == a {
                    found = true;
                    break;
                }
                if !seen.insert(id) {
                    continue;
                }
                if let Some((s, up)) = self.node(id)? {
                    if s < sa {
                        continue;
                    }
                    stack.extend(up);
                }
            }
        }
        self.ancestry.insert((a, b), found);
        Ok(found)
    }

    /// Whether a merge reachable from `at` resolved `change` away (it is recorded but not in effect).
    pub fn skipped_at(&mut self, change: ChangeId, at: ChangesetId) -> Result<bool> {
        let Some(merges) = self.skips.get(&change).cloned() else {
            return Ok(false);
        };
        for m in merges {
            if self.is_ancestor_or_self(m, at)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Merge changesets reachable from `at` that recorded `change` as not kept.
    pub fn merge_skips_at(&mut self, change: ChangeId, at: ChangesetId) -> Result<Vec<String>> {
        let Some(merges) = self.skips.get(&change).cloned() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for m in merges {
            if self.is_ancestor_or_self(m, at)? {
                out.push(m.to_string());
            }
        }
        out.sort();
        Ok(out)
    }

    /// All changes that set `unit`'s state, oldest first.
    pub fn unit_rows(&mut self, unit: Unit) -> Result<Rc<Vec<(i64, Change)>>> {
        if let Some(r) = self.unit_rows.get(&unit) {
            return Ok(r.clone());
        }
        let rows: Vec<(i64, Change)> = match unit {
            // A moved section keeps its id, so look it up by section in any concept.
            Unit::Section(s) => Ledger::changes_for_section(self.conn, s)?,
            Unit::Frontmatter(c) => Ledger::changes_for_concept(self.conn, c, None)?,
        }
        .into_iter()
        .filter(|(_, c)| unit_of(c) == Some(unit))
        .collect();
        let rows = Rc::new(rows);
        self.unit_rows.insert(unit, rows.clone());
        Ok(rows)
    }

    /// The versions that define `unit` at snapshot `at` (only changes strictly before
    /// `(at, before_ordinal)` when `before_ordinal` is set). Empty when the unit does not exist
    /// there (never created, or a section removed).
    ///
    /// Normally one version. Within one ref's history the surviving versions of a unit form a
    /// causal chain, because a concurrent edit is a merge conflict and the losing side is
    /// recorded as a skip. The exception is a byte-identical edit made on both sides of a merge
    /// (same commitment, no conflict): then both concurrent versions are heads, and the unit
    /// carries the union of their dependencies. Heads are ordered newest first.
    pub fn heads_at(
        &mut self,
        unit: Unit,
        at: ChangesetId,
        before_ordinal: Option<u32>,
    ) -> Result<Rc<Vec<Change>>> {
        let key = (unit, at, before_ordinal);
        if let Some(r) = self.heads_memo.get(&key) {
            return Ok(r.clone());
        }
        let rows = self.unit_rows(unit)?;
        let mut visible: Vec<(i64, &Change)> = Vec::new();
        for (seq, ch) in rows.iter() {
            if ch.changeset == at {
                if let Some(o) = before_ordinal {
                    if ch.ordinal >= o {
                        continue;
                    }
                }
            } else if !self.is_ancestor_or_self(ch.changeset, at)? {
                continue;
            }
            if self.skipped_at(ch.id, at)? {
                continue;
            }
            visible.push((*seq, ch));
        }
        visible.sort_by_key(|(seq, c)| std::cmp::Reverse((*seq, c.ordinal)));
        // Maximal elements, newest first: a version is a head unless it precedes a kept head.
        let mut heads: Vec<&Change> = Vec::new();
        for (_, c) in &visible {
            let mut superseded = false;
            for hd in &heads {
                let before = if c.changeset == hd.changeset {
                    c.ordinal < hd.ordinal
                } else {
                    self.is_ancestor_or_self(c.changeset, hd.changeset)?
                };
                if before {
                    superseded = true;
                    break;
                }
            }
            if !superseded {
                heads.push(c);
            }
        }
        let out: Vec<Change> = heads
            .into_iter()
            .filter(|c| c.op != Op::RemoveSection)
            .cloned()
            .collect();
        let rc = Rc::new(out);
        self.heads_memo.insert(key, rc.clone());
        Ok(rc)
    }

    /// The newest version of `section` at snapshot `at` (see [`History::heads_at`]); `None` when
    /// the section does not exist there.
    pub fn section_at(
        &mut self,
        section: SectionId,
        at: ChangesetId,
        before_ordinal: Option<u32>,
    ) -> Result<Option<Change>> {
        Ok(self
            .heads_at(Unit::Section(section), at, before_ordinal)?
            .first()
            .cloned())
    }

    /// The versions of the same unit that `version` replaced (two after an identical-content merge).
    pub fn predecessors(&mut self, version: &Change) -> Result<Rc<Vec<Change>>> {
        match unit_of(version) {
            Some(u) => self.heads_at(u, version.changeset, Some(version.ordinal)),
            None => Ok(Rc::new(Vec::new())),
        }
    }

    /// Whether `concept` had been created by snapshot `at`.
    pub fn concept_exists_at(&mut self, concept: ConceptId, at: ChangesetId) -> Result<bool> {
        for (_, ch) in Ledger::changes_for_concept(self.conn, concept, None)? {
            if self.is_ancestor_or_self(ch.changeset, at)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

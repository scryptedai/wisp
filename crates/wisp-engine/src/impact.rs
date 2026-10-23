//! `wisp impact <path>[#section]` (historical mode): which recorded work depends on a source?
//!
//! The forward direction of `wisp why`. A dependency edge is one of:
//!
//! - **input**: a change declared the item as an input (`pinned` to the snapshot it read, or
//!   `unversioned`);
//! - **inherited**: a new version of a section carries the dependencies of the version it
//!   replaced, unless the writer declared `inputs_complete`;
//! - **split / merge / copy**: a section created from another carries that section's
//!   dependencies (merge: the union), unless declared `inputs_complete`.
//!
//! Results are change versions reachable from the selected ref; pinned snapshots on other
//! branches are followed exactly as recorded, but dependents are never joined across unrelated
//! branches. Everything is metadata: no payload is read or decrypted, so erased sources, erased
//! intermediates, and erased actors still trace by opaque id. The command never writes; opening
//! a repository may complete a pending erase, and reading a ref may rebuild its hot tier.

use crate::repo::Repository;
use crate::snapshot::{is_section_version, unit_of, writes_section_content, History, Unit};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;
use wisp_model::*;
use wisp_store::{Hot, Ledger};

/// The exact message for an empty result. Absence from the graph is not evidence of independence.
pub const EMPTY_MESSAGE: &str = "No recorded dependents found within the selected scope.";

/// Default `--depth`: dependency hops from the source (inherited hops do not count).
pub const DEFAULT_DEPTH: usize = 3;

/// The starting point.
#[derive(Clone, Debug)]
pub struct Source {
    pub concept: ConceptId,
    pub section: Option<SectionId>,
    /// Label from the selected ref (live or retracted); `None` once erased or gone from the ref.
    pub path: Option<String>,
    /// `live`, `retracted`, `removed`, `erased`, or `not_on_ref`.
    pub state: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Pinned,
    Unversioned,
    Inherited,
    Split,
    Merge,
    Copy,
}

impl Link {
    pub fn as_str(self) -> &'static str {
        match self {
            Link::Pinned => "pinned",
            Link::Unversioned => "unversioned",
            Link::Inherited => "inherited",
            Link::Split => "split",
            Link::Merge => "merge",
            Link::Copy => "copy",
        }
    }
    fn counts_toward_depth(self) -> bool {
        self != Link::Inherited
    }
}

#[derive(Clone, Copy, Debug)]
enum Prev {
    Source,
    Node(usize),
}

/// How a node depends on the element before it.
#[derive(Clone, Debug)]
struct Edge {
    from: Prev,
    /// For a source element: the section the input named (or the source section).
    from_section: Option<SectionId>,
    /// For a source element: the version shown at the pinned snapshot, when known.
    from_version: Option<ChangeId>,
    link: Link,
    snapshot: Option<ChangesetId>,
    concept_level: bool,
}

#[derive(Clone, Debug)]
struct Node {
    change: Change,
    depth: usize,
    edge: Edge,
}

struct Walk {
    nodes: Vec<Node>,
    truncated: bool,
    ref_set: Rc<HashSet<ChangesetId>>,
}

/// Section content written on the ref with no declared inputs: dependents nobody recorded.
fn untracked_on(h: &History, ref_set: &HashSet<ChangesetId>) -> Result<usize> {
    let mut n = 0usize;
    for (_, c) in Ledger::changes_all(h.conn())? {
        if writes_section_content(c.op)
            && c.inputs.is_empty()
            && !c.inputs_complete
            && ref_set.contains(&c.changeset)
        {
            n += 1;
        }
    }
    Ok(n)
}

/// What a result is: section content, a concept's frontmatter, or another concept-level change.
fn kind_of(c: &Change) -> &'static str {
    unit_of(c).map(|u| u.kind()).unwrap_or("concept")
}

/// Indexes over the whole ledger, built once per query.
struct Index {
    consumers: Vec<Change>,
    /// input concept → (consumer index, input index)
    by_concept: HashMap<ConceptId, Vec<(usize, usize)>>,
    lineage: Vec<Change>,
    /// parent section → (lineage change index, link)
    by_parent: HashMap<SectionId, Vec<(usize, Link)>>,
}

impl Index {
    fn build(conn: &rusqlite::Connection) -> Result<Self> {
        let consumers: Vec<Change> = Ledger::changes_with_inputs(conn)?
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| c.op.is_content_change())
            .collect();
        let mut by_concept: HashMap<ConceptId, Vec<(usize, usize)>> = HashMap::new();
        for (ci, c) in consumers.iter().enumerate() {
            for (ii, i) in c.inputs.iter().enumerate() {
                // Asset and external-source inputs are not traced in this version.
                if let InputRef::Concept { concept, .. } = i {
                    by_concept.entry(*concept).or_default().push((ci, ii));
                }
            }
        }
        let lineage: Vec<Change> = Ledger::changes_with_lineage(conn)?
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| is_section_version(c.op))
            .collect();
        let mut by_parent: HashMap<SectionId, Vec<(usize, Link)>> = HashMap::new();
        for (li, c) in lineage.iter().enumerate() {
            match &c.lineage {
                Some(Lineage::SplitFrom { section }) => by_parent
                    .entry(*section)
                    .or_default()
                    .push((li, Link::Split)),
                Some(Lineage::CopiedFrom { section, .. }) => by_parent
                    .entry(*section)
                    .or_default()
                    .push((li, Link::Copy)),
                Some(Lineage::MergedFrom { sections }) => {
                    for s in sections {
                        by_parent.entry(*s).or_default().push((li, Link::Merge));
                    }
                }
                // A move keeps the section id; inheritance covers it.
                _ => {}
            }
        }
        Ok(Index {
            consumers,
            by_concept,
            lineage,
            by_parent,
        })
    }
}

fn input_parts(i: &InputRef) -> (Option<SectionId>, Option<ChangesetId>) {
    match i {
        InputRef::Concept { section, at, .. } => (*section, *at),
        _ => (None, None),
    }
}

/// Whether `v` was visible to a writer whose change is `d` (strictly earlier in its history).
fn visible_to(h: &mut History, v: &Change, d: &Change) -> Result<bool> {
    if v.changeset == d.changeset {
        return Ok(v.ordinal < d.ordinal);
    }
    h.is_ancestor_or_self(v.changeset, d.changeset)
}

/// Whether some version of `section` existed before change `d` in `d`'s history.
fn section_existed_before(h: &mut History, section: SectionId, d: &Change) -> Result<bool> {
    Ok(h.section_at(section, d.changeset, Some(d.ordinal))?
        .is_some()
        || {
            // A section later removed still counts if it existed earlier in d's history.
            let rows = h.unit_rows(Unit::Section(section))?;
            let mut found = false;
            for (_, c) in rows.iter() {
                if is_section_version(c.op) && visible_to(h, c, d)? {
                    found = true;
                    break;
                }
            }
            found
        })
}

/// Direct dependents of the source itself.
fn source_dependents(h: &mut History, ix: &Index, src: &Source) -> Result<Vec<(Change, Edge)>> {
    let mut out = Vec::new();
    for &(ci, ii) in ix.by_concept.get(&src.concept).into_iter().flatten() {
        let d = &ix.consumers[ci];
        let (s_i, at) = input_parts(&d.inputs[ii]);
        if let Some(s0) = src.section {
            if !(s_i == Some(s0) || s_i.is_none()) {
                continue;
            }
            if d.target.section() == Some(s0) {
                continue; // the source citing itself
            }
        } else if d.target.concept() == Some(src.concept) {
            continue; // part of the source concept
        }
        let probe = s_i.or(src.section);
        let (link, from_version) = match at {
            Some(cs) => match probe {
                Some(s) => match h.section_at(s, cs, None)? {
                    Some(v) => (Link::Pinned, Some(v.id)),
                    None => continue,
                },
                None => {
                    if !h.concept_exists_at(src.concept, cs)? {
                        continue;
                    }
                    (Link::Pinned, None)
                }
            },
            None => {
                let existed = match probe {
                    Some(s) => section_existed_before(h, s, d)?,
                    None => h.concept_exists_at(src.concept, d.changeset)?,
                };
                if !existed {
                    continue;
                }
                (Link::Unversioned, None)
            }
        };
        out.push((
            d.clone(),
            Edge {
                from: Prev::Source,
                from_section: probe,
                from_version,
                link,
                snapshot: at,
                concept_level: s_i.is_none(),
            },
        ));
    }
    // Sections created from the source by split, merge, or copy.
    let parents: Vec<SectionId> = match src.section {
        Some(s0) => vec![s0],
        None => ix
            .by_parent
            .keys()
            .copied()
            .filter(|p| {
                h.unit_rows(Unit::Section(*p))
                    .map(|rows| {
                        rows.iter()
                            .any(|(_, c)| c.target.concept() == Some(src.concept))
                    })
                    .unwrap_or(false)
            })
            .collect(),
    };
    for p in parents {
        for &(li, link) in ix.by_parent.get(&p).into_iter().flatten() {
            let d = &ix.lineage[li];
            if d.inputs_complete {
                continue;
            }
            if src.section.is_none() && d.target.concept() == Some(src.concept) {
                continue;
            }
            if src.section.is_some() && d.target.section() == src.section {
                continue;
            }
            let Some(v) = h.section_at(p, d.changeset, Some(d.ordinal))? else {
                continue;
            };
            out.push((
                d.clone(),
                Edge {
                    from: Prev::Source,
                    from_section: Some(p),
                    from_version: Some(v.id),
                    link,
                    snapshot: None,
                    concept_level: false,
                },
            ));
        }
    }
    Ok(out)
}

/// Dependents of an affected version through declared inputs and lineage (depth-counting hops).
fn node_dependents(
    h: &mut History,
    ix: &Index,
    v: &Change,
    vi: usize,
) -> Result<Vec<(Change, Edge)>> {
    let mut out = Vec::new();
    let Some(k) = v.target.concept() else {
        return Ok(out);
    };
    let t = v.target.section();
    let unit = unit_of(v);
    for &(ci, ii) in ix.by_concept.get(&k).into_iter().flatten() {
        let d = &ix.consumers[ci];
        if d.id == v.id {
            continue;
        }
        let (s_i, at) = input_parts(&d.inputs[ii]);
        // A section input matches that section; a whole-concept input matches every unit.
        match t {
            Some(t) if !(s_i == Some(t) || s_i.is_none()) => continue,
            None if s_i.is_some() => continue,
            _ => {}
        }
        let link = match at {
            Some(cs) => {
                let shown = match unit {
                    Some(u) => h.heads_at(u, cs, None)?.iter().any(|x| x.id == v.id),
                    None => h.is_ancestor_or_self(v.changeset, cs)?,
                };
                if !shown {
                    continue;
                }
                Link::Pinned
            }
            None => {
                if !visible_to(h, v, d)? {
                    continue;
                }
                Link::Unversioned
            }
        };
        out.push((
            d.clone(),
            Edge {
                from: Prev::Node(vi),
                from_section: None,
                from_version: None,
                link,
                snapshot: at,
                concept_level: s_i.is_none(),
            },
        ));
    }
    if let Some(t) = t {
        for &(li, link) in ix.by_parent.get(&t).into_iter().flatten() {
            let d = &ix.lineage[li];
            if d.inputs_complete || d.id == v.id {
                continue;
            }
            if !h
                .heads_at(Unit::Section(t), d.changeset, Some(d.ordinal))?
                .iter()
                .any(|x| x.id == v.id)
            {
                continue;
            }
            out.push((
                d.clone(),
                Edge {
                    from: Prev::Node(vi),
                    from_section: None,
                    from_version: None,
                    link,
                    snapshot: None,
                    concept_level: false,
                },
            ));
        }
    }
    Ok(out)
}

/// Later versions of the same unit (section, or the concept's frontmatter) that inherit `v`'s
/// dependencies: every version whose predecessors include `v`, unless it declared
/// `inputs_complete`.
fn inherited_successors(h: &mut History, v: &Change, vi: usize) -> Result<Vec<(Change, Edge)>> {
    let mut out = Vec::new();
    let Some(unit) = unit_of(v) else {
        return Ok(out);
    };
    if v.op == Op::RemoveSection {
        return Ok(out);
    }
    let rows = h.unit_rows(unit)?;
    for (_, u) in rows.iter() {
        if u.id == v.id || u.op == Op::RemoveSection || u.inputs_complete {
            continue;
        }
        if h.predecessors(u)?.iter().any(|p| p.id == v.id) {
            out.push((
                u.clone(),
                Edge {
                    from: Prev::Node(vi),
                    from_section: None,
                    from_version: None,
                    link: Link::Inherited,
                    snapshot: None,
                    concept_level: false,
                },
            ));
        }
    }
    Ok(out)
}

fn walk(
    h: &mut History,
    ix: &Index,
    src: &Source,
    head: Option<ChangesetId>,
    limit: usize,
) -> Result<Walk> {
    let ref_set = match head {
        Some(hd) => h.closure(hd)?,
        None => Rc::new(HashSet::new()),
    };
    let mut nodes: Vec<Node> = Vec::new();
    let mut seen: HashMap<ChangeId, usize> = HashMap::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut truncated = false;

    for (c, edge) in source_dependents(h, ix, src)? {
        if seen.contains_key(&c.id) {
            continue;
        }
        seen.insert(c.id, nodes.len());
        queue.push_back(nodes.len());
        nodes.push(Node {
            change: c,
            depth: 1,
            edge,
        });
    }
    // 0-1 BFS: inherited hops keep the depth (pushed to the front), dependency hops add one.
    while let Some(ni) = queue.pop_front() {
        let (change, depth) = (nodes[ni].change.clone(), nodes[ni].depth);
        for (u, edge) in inherited_successors(h, &change, ni)? {
            if seen.contains_key(&u.id) {
                continue;
            }
            seen.insert(u.id, nodes.len());
            queue.push_front(nodes.len());
            nodes.push(Node {
                change: u,
                depth,
                edge,
            });
        }
        let deps = node_dependents(h, ix, &change, ni)?;
        if depth >= limit {
            if deps.iter().any(|(d, _)| !seen.contains_key(&d.id)) {
                truncated = true;
            }
            continue;
        }
        for (d, edge) in deps {
            if seen.contains_key(&d.id) {
                continue;
            }
            debug_assert!(edge.link.counts_toward_depth());
            seen.insert(d.id, nodes.len());
            queue.push_back(nodes.len());
            nodes.push(Node {
                change: d,
                depth: depth + 1,
                edge,
            });
        }
    }

    Ok(Walk {
        nodes,
        truncated,
        ref_set,
    })
}

/// Resolve the starting point: live on the ref, retracted on the ref, or an opaque concept id
/// known only from ledger metadata (erased, removed, or not on this ref).
pub fn resolve_source(repo: &mut Repository, ref_name: &str, spec: &str) -> Result<Source> {
    let (path_part, sec_part) = match spec.split_once('#') {
        Some((p, s)) => (p, Some(s)),
        None => (spec, None),
    };
    let section = sec_part
        .map(|s| {
            s.parse::<SectionId>()
                .map_err(|_| WispError::usage(format!("bad section id {s:?}")))
        })
        .transpose()?;
    let cpath = concept_path_of(path_part);
    let as_id = cpath.parse::<ConceptId>().ok();
    let (concept, path, state, section_live) = {
        let conn = repo.hot(ref_name)?.conn();
        let live = match as_id {
            Some(id) => Hot::concept(conn, id)?,
            None => Hot::concept_by_path(conn, &cpath)?.or(Hot::concept_by_alias(conn, &cpath)?),
        };
        let found = match live {
            Some(c) => Some((c.concept, Some(c.path), "live")),
            None => Hot::retracted_all(conn)?
                .into_iter()
                .find(|(id, p)| *p == cpath || Some(*id) == as_id)
                .map(|(id, p)| (id, Some(p), "retracted")),
        };
        let (concept, path, state) = match (found, as_id) {
            (Some(f), _) => f,
            (None, Some(id)) => (id, None, "not_live"),
            (None, None) => {
                return Err(WispError::not_found(format!(
                    "no concept at {cpath} on {ref_name}; an erased or removed source can be named by its concept id"
                )))
            }
        };
        let section_live = section.map(|s| {
            Hot::section(conn, s)
                .ok()
                .flatten()
                .map(|st| st.concept == concept)
                .unwrap_or(false)
        });
        (concept, path, state, section_live)
    };
    let conn = repo.ledger.conn();
    let concept_rows = Ledger::changes_for_concept(conn, concept, None)?;
    if concept_rows.is_empty() {
        return Err(WispError::not_found(format!(
            "no concept {concept} in the ledger"
        )));
    }
    if let Some(s) = section {
        let srows = Ledger::changes_for_section(conn, s)?;
        if !srows
            .iter()
            .any(|(_, c)| c.target.concept() == Some(concept))
        {
            return Err(WispError::not_found(format!(
                "section {s} never belonged to concept {concept}"
            )));
        }
    }
    let erased = match section {
        Some(s) => Ledger::scope_of_section(conn, s)?
            .map(|r| r.erased)
            .unwrap_or(false),
        None => Ledger::scopes_of_concept(conn, concept)?
            .iter()
            .any(|r| r.erased && matches!(r.kind, ScopeKind::Concept { .. })),
    };
    let state = if erased {
        "erased"
    } else if state == "live" && section_live == Some(false) {
        "removed"
    } else if state == "not_live" {
        let mut h = History::new(conn)?;
        let on_ref = match repo.head(ref_name)? {
            Some(hd) => {
                let set = h.closure(hd)?;
                concept_rows.iter().any(|(_, c)| set.contains(&c.changeset))
            }
            None => false,
        };
        if on_ref {
            "removed"
        } else {
            "not_on_ref"
        }
    } else {
        state
    };
    Ok(Source {
        concept,
        section,
        path: if erased { None } else { path },
        state,
    })
}

/// `wisp impact <path|concept-id>[#section] [--depth N] [--ref R]` (historical mode).
pub fn impact(repo: &mut Repository, ref_name: &str, spec: &str, depth: usize) -> Result<Value> {
    if depth == 0 {
        return Err(WispError::usage("--depth must be at least 1"));
    }
    let src = resolve_source(repo, ref_name, spec)?;
    let head = repo.head(ref_name)?;
    let (walk, skipped, seqs, untracked) = {
        let mut h = History::new(repo.ledger.conn())?;
        let ix = Index::build(h.conn())?;
        let w = walk(&mut h, &ix, &src, head, depth)?;
        let untracked = untracked_on(&h, &w.ref_set)?;
        let mut seqs: HashMap<ChangesetId, i64> = HashMap::new();
        let mut skipped: HashSet<ChangeId> = HashSet::new();
        for n in &w.nodes {
            if let Some(s) = h.seq(n.change.changeset)? {
                seqs.insert(n.change.changeset, s);
            }
            if let Some(hd) = head {
                if h.skipped_at(n.change.id, hd)? {
                    skipped.insert(n.change.id);
                }
            }
        }
        (w, skipped, seqs, untracked)
    };

    // Labels from the selected ref only (never from payloads).
    let mut labels: HashMap<ConceptId, String> = HashMap::new();
    {
        let conn = repo.hot(ref_name)?.conn();
        let retracted: HashMap<ConceptId, String> = Hot::retracted_all(conn)?.into_iter().collect();
        let mut concepts: HashSet<ConceptId> = walk
            .nodes
            .iter()
            .filter_map(|n| n.change.target.concept())
            .collect();
        concepts.remove(&src.concept);
        for c in concepts {
            if let Some(st) = Hot::concept(conn, c)? {
                labels.insert(c, st.path);
            } else if let Some(p) = retracted.get(&c) {
                labels.insert(c, p.clone());
            }
        }
    }
    let label = |c: Option<ConceptId>| -> Value {
        match c {
            Some(c) if c == src.concept => json!(src.path),
            Some(c) => json!(labels.get(&c)),
            None => Value::Null,
        }
    };

    let via_of = |ni: usize| -> Vec<Value> {
        let mut chain: Vec<Value> = Vec::new();
        let mut edge = &walk.nodes[ni].edge;
        loop {
            let mut el = match edge.from {
                Prev::Source => json!({
                    "concept_id": src.concept.to_string(),
                    "section_id": edge.from_section.map(|s| s.to_string()),
                    "version": edge.from_version.map(|v| v.to_string()),
                    "path": label(Some(src.concept)),
                    "source": true,
                }),
                Prev::Node(p) => {
                    let c = &walk.nodes[p].change;
                    json!({
                        "concept_id": c.target.concept().map(|x| x.to_string()),
                        "section_id": c.target.section().map(|x| x.to_string()),
                        "kind": kind_of(c),
                        "version": c.id.to_string(),
                        "path": label(c.target.concept()),
                    })
                }
            };
            el["link"] = json!(edge.link.as_str());
            el["snapshot"] = json!(edge.snapshot.map(|s| s.to_string()));
            if matches!(edge.link, Link::Pinned | Link::Unversioned) {
                el["concept_level"] = json!(edge.concept_level);
            }
            chain.push(el);
            match edge.from {
                Prev::Source => break,
                Prev::Node(p) => edge = &walk.nodes[p].edge,
            }
        }
        chain.reverse();
        chain
    };

    let mut order: Vec<usize> = (0..walk.nodes.len())
        .filter(|&i| walk.ref_set.contains(&walk.nodes[i].change.changeset))
        .collect();
    order.sort_by_key(|&i| {
        let c = &walk.nodes[i].change;
        (
            walk.nodes[i].depth,
            seqs.get(&c.changeset).copied().unwrap_or(i64::MAX),
            c.ordinal,
        )
    });
    let mut cs_meta: HashMap<ChangesetId, (ActorId, String)> = HashMap::new();
    let mut results = Vec::with_capacity(order.len());
    let mut unversioned = 0usize;
    for i in order {
        let n = &walk.nodes[i];
        let c = &n.change;
        let (actor_id, at) = match cs_meta.get(&c.changeset) {
            Some(m) => m.clone(),
            None => {
                let row = Ledger::get_changeset(repo.ledger.conn(), c.changeset)?
                    .ok_or_else(|| WispError::integrity("impact: missing changeset"))?;
                let m = (row.changeset.actor_id, row.changeset.at.clone());
                cs_meta.insert(c.changeset, m.clone());
                m
            }
        };
        if n.edge.link == Link::Unversioned {
            unversioned += 1;
        }
        results.push(json!({
            "concept_id": c.target.concept().map(|x| x.to_string()),
            "path": label(c.target.concept()),
            "section_id": c.target.section().map(|x| x.to_string()),
            "kind": kind_of(c),
            "version": c.id.to_string(),
            "op": c.op.as_str(),
            "changeset": c.changeset.to_string(),
            "actor": repo.actor_display(actor_id),
            "actor_id": actor_id.to_string(),
            "at": at,
            "depth": n.depth,
            "link": n.edge.link.as_str(),
            "inputs_complete": c.inputs_complete,
            "merge_skipped": skipped.contains(&c.id),
            "via": via_of(i),
        }));
    }
    let message = if results.is_empty() {
        json!(EMPTY_MESSAGE)
    } else {
        Value::Null
    };
    Ok(json!({
        "source": {
            "concept_id": src.concept.to_string(),
            "section_id": src.section.map(|s| s.to_string()),
            "path": src.path,
            "state": src.state,
        },
        "ref": ref_name,
        "head": head.map(|h| h.to_string()),
        "mode": "historical",
        "depth_limit": depth,
        "truncated": walk.truncated,
        "unversioned_links": unversioned,
        "untracked_content_changes": untracked,
        "results": results,
        "message": message,
    }))
}

/// Recorded dependents of erased scopes on every ref, grouped by ref. Erasure destroys the key
/// for every branch, so its report is deliberately repository-wide (`wisp impact` stays
/// ref-scoped). Metadata only: safe to run during open-time erase recovery.
pub struct AllRefs {
    /// ref name → dependents reachable from that ref (refs with none are omitted).
    pub by_ref: BTreeMap<String, Vec<Value>>,
    /// Every dependent concept on any ref (the shape of erase's `flagged`).
    pub concepts: Vec<ConceptId>,
    pub truncated: bool,
}

pub fn dependents_all_refs(repo: &Repository, scopes: &[ScopeId], depth: usize) -> Result<AllRefs> {
    let conn = repo.ledger.conn();
    let mut sources = Vec::new();
    for s in scopes {
        let Some(row) = Ledger::scope(conn, *s)? else {
            continue;
        };
        let (concept, section) = match row.kind {
            ScopeKind::Concept { concept } => (concept, None),
            ScopeKind::Section { concept, section } => (concept, Some(section)),
            _ => continue,
        };
        sources.push(Source {
            concept,
            section,
            path: None,
            state: "erased",
        });
    }
    let mut by_ref: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut concepts: BTreeSet<ConceptId> = BTreeSet::new();
    let mut truncated = false;
    if sources.is_empty() {
        return Ok(AllRefs {
            by_ref,
            concepts: vec![],
            truncated,
        });
    }
    let mut h = History::new(conn)?;
    let ix = Index::build(conn)?;
    for (ref_name, head) in Ledger::refs(conn)? {
        let mut seen: HashSet<ChangeId> = HashSet::new();
        let mut rows: Vec<Value> = Vec::new();
        for src in &sources {
            let w = walk(&mut h, &ix, src, Some(head), depth)?;
            truncated |= w.truncated;
            for n in &w.nodes {
                let c = &n.change;
                if !w.ref_set.contains(&c.changeset) || !seen.insert(c.id) {
                    continue;
                }
                if let Some(k) = c.target.concept() {
                    concepts.insert(k);
                }
                rows.push(json!({
                    "concept_id": c.target.concept().map(|x| x.to_string()),
                    "section_id": c.target.section().map(|x| x.to_string()),
                    "kind": kind_of(c),
                    "version": c.id.to_string(),
                    "depth": n.depth,
                    "link": n.edge.link.as_str(),
                    "source_concept_id": src.concept.to_string(),
                    "source_section_id": src.section.map(|x| x.to_string()),
                }));
            }
        }
        if !rows.is_empty() {
            by_ref.insert(ref_name, rows);
        }
    }
    Ok(AllRefs {
        by_ref,
        concepts: concepts.into_iter().collect(),
        truncated,
    })
}

//! Ref merge (§11.5): fast-forward and divergent conflict detection.

use crate::checkout;
use crate::commit::record_merge;
use crate::query;
use crate::repo::Repository;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use wisp_model::*;
use wisp_store::Ledger;

/// Merge `source` into `into`. Fast-forwards when `into` is an ancestor of `source`.
pub fn merge_refs(
    repo: &mut Repository,
    source: &str,
    into: &str,
    resolutions: Option<&std::path::Path>,
    abort: bool,
) -> Result<Value> {
    if abort {
        return Err(WispError::usage("merge --abort is not used yet"));
    }
    if resolutions.is_some() {
        return Err(WispError::not_implemented(
            "merge --resolutions (apply conflict file)",
        ));
    }
    if Ledger::head(repo.ledger.conn(), source)?.is_none() {
        return Err(WispError::not_found(format!("no ref {source}")));
    }
    if Ledger::head(repo.ledger.conn(), into)?.is_none() {
        return Err(WispError::not_found(format!("no ref {into}")));
    }
    let into_head = repo
        .head(into)?
        .ok_or_else(|| WispError::usage(format!("ref {into} has no commits")))?;
    let source_head = repo
        .head(source)?
        .ok_or_else(|| WispError::usage(format!("ref {source} has no commits")))?;

    if into_head == source_head {
        return Ok(json!({
            "status": "already_up_to_date",
            "into": into,
            "head": into_head.to_string(),
        }));
    }

    let conn = repo.ledger.conn();
    if is_ancestor(conn, into_head, source_head)? {
        fast_forward(repo, into, source, into_head, source_head)?;
        return Ok(json!({
            "status": "fast_forward",
            "into": into,
            "from": into_head.to_string(),
            "head": source_head.to_string(),
        }));
    }
    if is_ancestor(conn, source_head, into_head)? {
        return Ok(json!({
            "status": "already_up_to_date",
            "into": into,
            "head": into_head.to_string(),
        }));
    }

    let base = merge_base(conn, into_head, source_head)?
        .ok_or_else(|| WispError::conflict("no common ancestor", json!([])))?;
    let into_cs = changesets_since(conn, into_head, base);
    let source_cs = changesets_since(conn, source_head, base);
    let into_keys = scope_keys_for_changesets(conn, &into_cs)?;
    let source_keys = scope_keys_for_changesets(conn, &source_cs)?;
    let conflicts = divergent_conflicts(&into_keys, &source_keys);
    if !conflicts.is_empty() {
        return Err(WispError::conflict(
            "merge conflicts",
            json!({ "type": "merge_conflict", "base": base.to_string(), "conflicts": conflicts }),
        ));
    }

    let merge_id = record_merge(repo, into, into_head, source_head)?;
    repo.hots.remove(into);
    crate::apply::rebuild_hot(repo, into)?;
    if query::current_ref(repo) == into {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, into, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(json!({
        "status": "merged",
        "into": into,
        "source": source,
        "base": base.to_string(),
        "merge": merge_id.to_string(),
        "head": merge_id.to_string(),
        "changesets_replayed": source_cs.len(),
    }))
}

fn fast_forward(
    repo: &mut Repository,
    into: &str,
    _source: &str,
    _old: ChangesetId,
    new_head: ChangesetId,
) -> Result<()> {
    let tx = repo.ledger.begin_immediate()?;
    Ledger::set_head(&tx, into, new_head)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    repo.hots.remove(into);
    repo.hot(into)?;
    if query::current_ref(repo) == into {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, into, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(())
}

pub fn is_ancestor(conn: &rusqlite::Connection, ancestor: ChangesetId, desc: ChangesetId) -> Result<bool> {
    if ancestor == desc {
        return Ok(true);
    }
    let mut stack = vec![desc];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if id == ancestor {
            return Ok(true);
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(row) = Ledger::get_changeset(conn, id)? {
            if let Some(b) = row.changeset.based_on {
                stack.push(b);
            }
            for p in &row.changeset.parents {
                stack.push(*p);
            }
        }
    }
    Ok(false)
}

fn merge_base(
    conn: &rusqlite::Connection,
    a: ChangesetId,
    b: ChangesetId,
) -> Result<Option<ChangesetId>> {
    let a_anc = ancestry_with_seq(conn, a)?;
    let b_anc = ancestry_with_seq(conn, b)?;
    let mut best: Option<(i64, ChangesetId)> = None;
    for (id, seq) in a_anc {
        if let Some(seq_b) = b_anc.get(&id) {
            let s = seq.min(*seq_b);
            if best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, id));
            }
        }
    }
    Ok(best.map(|(_, id)| id))
}

fn ancestry_with_seq(
    conn: &rusqlite::Connection,
    start: ChangesetId,
) -> Result<HashMap<ChangesetId, i64>> {
    let mut out = HashMap::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if out.contains_key(&id) {
            continue;
        }
        let Some(row) = Ledger::get_changeset(conn, id)? else {
            continue;
        };
        out.insert(id, row.seq);
        if let Some(b) = row.changeset.based_on {
            stack.push(b);
        }
        for p in &row.changeset.parents {
            stack.push(*p);
        }
    }
    Ok(out)
}

/// Changesets on the `based_on` chain from `base` (exclusive) to `head` (inclusive), oldest first.
pub fn changesets_since(conn: &rusqlite::Connection, head: ChangesetId, base: ChangesetId) -> Vec<ChangesetId> {
    let mut path = Vec::new();
    let mut cur = Some(head);
    while let Some(id) = cur {
        if id == base {
            break;
        }
        path.push(id);
        cur = Ledger::get_changeset(conn, id)
            .ok()
            .flatten()
            .and_then(|r| r.changeset.based_on);
    }
    path.reverse();
    path
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ScopeKey {
    Section(ConceptId, SectionId),
    Frontmatter(ConceptId, String),
    ConceptPath(ConceptId),
    Source(ConceptId, String),
    Asset(AssetId),
    Other(String),
}

fn scope_key(c: &Change) -> ScopeKey {
    match &c.target {
        Target::Section { concept, section } => ScopeKey::Section(*concept, *section),
        Target::FrontmatterKey { concept, key_commit } => {
            ScopeKey::Frontmatter(*concept, hex::encode(key_commit.0))
        }
        Target::Concept { concept } => ScopeKey::ConceptPath(*concept),
        Target::Source { concept, source } => ScopeKey::Source(*concept, source.clone()),
        Target::Asset { asset } => ScopeKey::Asset(*asset),
        other => ScopeKey::Other(format!("{other:?}")),
    }
}

fn scope_keys_for_changesets(
    conn: &rusqlite::Connection,
    ids: &[ChangesetId],
) -> Result<HashMap<ScopeKey, ChangeId>> {
    let mut map = HashMap::new();
    for cs in ids {
        for c in Ledger::changes_of(conn, *cs)? {
            if c.op == Op::Verify {
                continue;
            }
            let key = scope_key(&c);
            map.insert(key, c.id);
        }
    }
    Ok(map)
}

fn divergent_conflicts(
    into: &HashMap<ScopeKey, ChangeId>,
    source: &HashMap<ScopeKey, ChangeId>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (key, theirs) in source {
        if let Some(ours) = into.get(key) {
            if ours != theirs {
                out.push(json!({
                    "type": "scope_conflict",
                    "scope": format!("{key:?}"),
                    "ours": ours.to_string(),
                    "theirs": theirs.to_string(),
                }));
            }
        }
    }
    out
}

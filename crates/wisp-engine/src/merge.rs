//! Ref merge (§11.5): fast-forward and divergent conflict detection.

use crate::checkout;
use crate::commit::record_merge;
use crate::query;
use crate::repo::Repository;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use wisp_model::*;
use wisp_store::Ledger;

const MERGE_STATE_FILE: &str = "merge.json";

fn merge_skip_path(repo: &Repository, ref_name: &str) -> PathBuf {
    repo.wisp_dir
        .join("cache")
        .join("refs")
        .join(ref_name)
        .join("merge-skip.json")
}

/// Skip list for a merge head (hot rebuild / hot check must match).
pub fn merge_skips_for_head(
    repo: &Repository,
    ref_name: &str,
    head: ChangesetId,
) -> Option<HashSet<ChangeId>> {
    let path = merge_skip_path(repo, ref_name);
    let Ok(text) = fs::read_to_string(&path) else {
        return None;
    };
    let v: Value = serde_json::from_str(&text).ok()?;
    if v.get("head").and_then(|h| h.as_str()) != Some(head.to_string().as_str()) {
        return None;
    }
    let mut out = HashSet::new();
    for s in v["skip"].as_array().into_iter().flatten() {
        if let Some(id) = s.as_str() {
            if let Ok(c) = ChangeId::from_str(id) {
                out.insert(c);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn persist_merge_skips(
    repo: &Repository,
    ref_name: &str,
    head: ChangesetId,
    skip: &HashSet<ChangeId>,
) -> Result<()> {
    let path = merge_skip_path(repo, ref_name);
    if skip.is_empty() {
        if path.is_file() {
            fs::remove_file(&path).ok();
        }
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| WispError::internal(e.to_string()))?;
    let skip_list: Vec<String> = skip.iter().map(|c| c.to_string()).collect();
    let v = json!({ "head": head.to_string(), "skip": skip_list });
    fs::write(&path, serde_json::to_vec_pretty(&v)?).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

pub fn clear_merge_skips(repo: &Repository, ref_name: &str) -> Result<()> {
    let path = merge_skip_path(repo, ref_name);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| WispError::internal(e.to_string()))?;
    }
    Ok(())
}

/// Merge `source` into `into`. Fast-forwards when `into` is an ancestor of `source`.
pub fn merge_refs(
    repo: &mut Repository,
    source: &str,
    into: &str,
    resolutions: Option<&Path>,
    abort: bool,
) -> Result<Value> {
    if abort {
        clear_merge_state(repo)?;
        return Ok(json!({ "status": "aborted" }));
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
        clear_merge_state(repo)?;
        return Ok(json!({
            "status": "already_up_to_date",
            "into": into,
            "head": into_head.to_string(),
        }));
    }

    let conn = repo.ledger.conn();
    if is_ancestor(conn, into_head, source_head)? {
        clear_merge_skips(repo, into)?;
        fast_forward(repo, into, source, into_head, source_head)?;
        clear_merge_state(repo)?;
        return Ok(json!({
            "status": "fast_forward",
            "into": into,
            "from": into_head.to_string(),
            "head": source_head.to_string(),
        }));
    }
    if is_ancestor(conn, source_head, into_head)? {
        clear_merge_state(repo)?;
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
    let conflicts = divergent_conflicts(conn, &into_keys, &source_keys)?;
    if !conflicts.is_empty() {
        if resolutions.is_none() {
            write_merge_state(
                repo,
                source,
                into,
                base,
                into_head,
                source_head,
                &conflicts,
            )?;
            return Err(WispError::conflict(
                "merge conflicts",
                json!({
                    "type": "merge_conflict",
                    "base": base.to_string(),
                    "conflicts": conflicts,
                    "state": merge_state_path(repo).display().to_string(),
                }),
            ));
        }
        let skip = skip_changes_from_resolutions(&conflicts, resolutions.unwrap())?;
        let merge_id = record_merge(repo, into, into_head, source_head)?;
        persist_merge_skips(repo, into, merge_id, &skip)?;
        repo.hots.remove(into);
        crate::apply::rebuild_hot_skipping(repo, into, &skip)?;
        finish_merge_checkout(repo, into)?;
        clear_merge_state(repo)?;
        return Ok(json!({
            "status": "merged",
            "into": into,
            "source": source,
            "base": base.to_string(),
            "merge": merge_id.to_string(),
            "head": merge_id.to_string(),
            "resolved": conflicts.len(),
        }));
    }

    let merge_id = record_merge(repo, into, into_head, source_head)?;
    clear_merge_skips(repo, into)?;
    repo.hots.remove(into);
    crate::apply::rebuild_hot(repo, into)?;
    finish_merge_checkout(repo, into)?;
    clear_merge_state(repo)?;
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

fn finish_merge_checkout(repo: &mut Repository, into: &str) -> Result<()> {
    if query::current_ref(repo) == into {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, into, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(())
}

fn merge_state_path(repo: &Repository) -> PathBuf {
    repo.wisp_dir.join(MERGE_STATE_FILE)
}

fn write_merge_state(
    repo: &Repository,
    source: &str,
    into: &str,
    base: ChangesetId,
    into_head: ChangesetId,
    source_head: ChangesetId,
    conflicts: &[Value],
) -> Result<()> {
    let v = json!({
        "source": source,
        "into": into,
        "base": base.to_string(),
        "into_head": into_head.to_string(),
        "source_head": source_head.to_string(),
        "conflicts": conflicts,
    });
    fs::write(merge_state_path(repo), serde_json::to_vec_pretty(&v)?).map_err(|e| {
        WispError::internal(format!("write merge state: {e}"))
    })?;
    Ok(())
}

pub fn clear_merge_state(repo: &Repository) -> Result<()> {
    let p = merge_state_path(repo);
    if p.is_file() {
        fs::remove_file(&p).map_err(|e| WispError::internal(format!("clear merge state: {e}")))?;
    }
    Ok(())
}

/// Pending merge metadata from `.wisp/merge.json` after a conflicted `wisp merge`.
pub fn read_merge_state(repo: &Repository) -> Result<Option<Value>> {
    let p = merge_state_path(repo);
    if !p.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&p).map_err(|e| WispError::internal(e.to_string()))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| WispError::internal(format!("parse merge.json: {e}")))?;
    Ok(Some(v))
}

#[derive(Debug, Deserialize)]
struct ResolutionFile {
    resolutions: Vec<ResolutionEntry>,
}

#[derive(Debug, Deserialize)]
struct ResolutionEntry {
    ours: String,
    theirs: String,
    take: String,
}

fn skip_changes_from_resolutions(
    conflicts: &[Value],
    path: &Path,
) -> Result<HashSet<ChangeId>> {
    let text = fs::read_to_string(path)
        .map_err(|e| WispError::usage(format!("read resolutions: {e}")))?;
    let file: ResolutionFile = serde_json::from_str(&text)
        .map_err(|e| WispError::usage(format!("parse resolutions: {e}")))?;
    let mut skip = HashSet::new();
    for c in conflicts {
        let ours_s = c["ours"].as_str().unwrap_or_default();
        let theirs_s = c["theirs"].as_str().unwrap_or_default();
        let entry = file
            .resolutions
            .iter()
            .find(|r| r.ours == ours_s && r.theirs == theirs_s)
            .ok_or_else(|| {
                WispError::usage(format!(
                    "resolutions missing entry for ours={ours_s} theirs={theirs_s}"
                ))
            })?;
        let ours = ChangeId::from_str(ours_s)
            .map_err(|_| WispError::usage("invalid ours change id in conflict"))?;
        let theirs = ChangeId::from_str(theirs_s)
            .map_err(|_| WispError::usage("invalid theirs change id in conflict"))?;
        match entry.take.as_str() {
            "ours" => skip.insert(theirs),
            "theirs" => skip.insert(ours),
            other => {
                return Err(WispError::usage(format!(
                    "take must be \"ours\" or \"theirs\", got {other:?}"
                )));
            }
        };
    }
    Ok(skip)
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
    finish_merge_checkout(repo, into)?;
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
    conn: &rusqlite::Connection,
    into: &HashMap<ScopeKey, ChangeId>,
    source: &HashMap<ScopeKey, ChangeId>,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (key, theirs) in source {
        if let Some(ours) = into.get(key) {
            if ours != theirs && !changes_same_commitment(conn, *ours, *theirs)? {
                out.push(json!({
                    "type": "scope_conflict",
                    "scope": format!("{key:?}"),
                    "ours": ours.to_string(),
                    "theirs": theirs.to_string(),
                }));
            }
        }
    }
    Ok(out)
}

/// §11.5: same section/frontmatter with the same payload commitment is not a conflict.
fn changes_same_commitment(
    conn: &rusqlite::Connection,
    ours: ChangeId,
    theirs: ChangeId,
) -> Result<bool> {
    let a = Ledger::get_change(conn, ours)?
        .ok_or_else(|| WispError::internal("merge conflict scan: missing change"))?;
    let b = Ledger::get_change(conn, theirs)?
        .ok_or_else(|| WispError::internal("merge conflict scan: missing change"))?;
    if a.op != b.op || a.target != b.target {
        return Ok(false);
    }
    Ok(match (&a.payload, &b.payload) {
        (Some(pa), Some(pb)) => pa.commit == pb.commit,
        (None, None) => true,
        _ => false,
    })
}

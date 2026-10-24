//! Lifecycle: retract/restore, erasure groups, and the erase procedure (§10.4–10.6).

use crate::checkout::{self, WorktreeLock};
use crate::commit::{commit, CommitRequest, PlannedOp, Precondition};
use crate::payloads::*;
use crate::query;
use crate::repo::{Repository, MAIN_REF};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wisp_model::*;
use wisp_store::{Hot, Ledger, Tombstone};

/// Resolve `<path>[#section]` on a ref to a concept (and optional section).
pub fn resolve_target(
    repo: &mut Repository,
    ref_name: &str,
    spec: &str,
) -> Result<(wisp_store::ConceptState, Option<SectionId>)> {
    let (path_part, section_part) = match spec.split_once('#') {
        Some((p, s)) => (p, Some(s)),
        None => (spec, None),
    };
    let section = match section_part {
        Some(s) => Some(
            s.parse::<SectionId>()
                .map_err(|_| WispError::usage(format!("bad section id {s:?}")))?,
        ),
        None => None,
    };
    let path = concept_path_of(path_part);
    let hot = repo.hot(ref_name)?;
    let conn = hot.conn();
    let concept = if let Ok(id) = path.parse::<ConceptId>() {
        Hot::concept(conn, id)?
    } else {
        Hot::concept_by_path(conn, &path)?.or(Hot::concept_by_alias(conn, &path)?)
    };
    let concept = concept.ok_or_else(|| WispError::not_found(format!("no concept at {path}")))?;
    if let Some(s) = section {
        let st = Hot::section(conn, s)?
            .ok_or_else(|| WispError::not_found(format!("no section {s} in {path}")))?;
        if st.concept != concept.concept {
            return Err(WispError::not_found(format!(
                "section {s} belongs to another concept"
            )));
        }
    }
    Ok((concept, section))
}

pub fn retract(
    repo: &mut Repository,
    ref_name: &str,
    path: &str,
    reason: Reason,
    note: Option<String>,
    actor: &str,
) -> Result<Value> {
    let (c, _) = resolve_target(repo, ref_name, path)?;
    let mut op = PlannedOp::new(Op::Retract, Target::Concept { concept: c.concept }, c.scope)
        .with_precondition(Precondition::ConceptLive(c.concept))
        .with_path(format!("{}.md", c.path));
    op.reason = Some(reason);
    op.note = note;
    op.policy.concept_type = c.type_.clone();
    let mut req = CommitRequest::new(ref_name, actor);
    req.ops = vec![op];
    let out = commit(repo, req)?;
    rewrite_tree_if_main(repo, ref_name)?;
    // Recorded direct dependents on this ref, read-only (the same list shape as erase's `flagged`).
    let impact = crate::impact::impact(repo, ref_name, &c.concept.to_string(), 1)?;
    let mut flagged: Vec<String> = impact["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["concept_id"].as_str().map(|s| s.to_string()))
        .collect();
    flagged.sort();
    flagged.dedup();
    Ok(json!({
        "changeset": out.changeset.to_string(),
        "retracted": c.path,
        "concept": c.concept.to_string(),
        "flagged": flagged,
        "flagged_truncated": impact["truncated"],
    }))
}

pub fn restore(repo: &mut Repository, ref_name: &str, path: &str, actor: &str) -> Result<Value> {
    let p = concept_path_of(path);
    repo.hot(ref_name)?;
    let conn = repo.hot(ref_name)?.conn();
    let retracted = Hot::retracted_all(conn)?;
    let (cid, _) = retracted
        .into_iter()
        .find(|(id, rp)| rp == &p || id.to_string() == p)
        .ok_or_else(|| WispError::not_found(format!("{p} is not retracted")))?;
    let scope = Ledger::scopes_of_concept(repo.ledger.conn(), cid)?
        .into_iter()
        .find(|s| matches!(s.kind, ScopeKind::Concept { .. }))
        .map(|s| s.id)
        .ok_or_else(|| WispError::internal("concept scope missing"))?;
    let op = PlannedOp::new(Op::Restore, Target::Concept { concept: cid }, scope)
        .with_precondition(Precondition::ConceptRetracted(cid))
        .with_path(format!("{p}.md"));
    let mut req = CommitRequest::new(ref_name, actor);
    req.ops = vec![op];
    let out = commit(repo, req)?;
    rewrite_tree_if_main(repo, ref_name)?;
    Ok(json!({ "changeset": out.changeset.to_string(), "restored": p }))
}

fn rewrite_tree_if_main(repo: &mut Repository, ref_name: &str) -> Result<()> {
    if ref_name == MAIN_REF {
        let root = repo.root.clone();
        let idx = checkout::write_tree(repo, ref_name, &root, false, false, true)?;
        idx.save(&checkout::checkout_idx_path(repo, &root))?;
    }
    Ok(())
}

// ---- groups ----

pub fn group_create(repo: &mut Repository, name: &str) -> Result<Value> {
    if find_group(repo, name)?.is_some() {
        return Err(WispError::usage(format!("group {name} exists")));
    }
    let id = GroupId::new();
    let label =
        repo.prepare_payload(repo.repo_scope(), PayloadKind::GroupLabel, name.as_bytes())?;
    let tx = repo.ledger.begin_immediate()?;
    Ledger::insert_payload_row(&tx, label.reference.id, label.scope, label.kind, label.size)?;
    if let Some(ct) = &label.ciphertext {
        Ledger::stage_payload(&tx, label.reference.id, ct)?;
    }
    Ledger::insert_group(&tx, id, &label.reference)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    repo.maybe_backpressure_cold_flush()?;
    repo.notify_cold_flush();
    repo.flush_cold_payloads()?;
    Ok(json!({ "group": id.to_string(), "name": name }))
}

pub fn find_group(repo: &Repository, name: &str) -> Result<Option<GroupId>> {
    for (id, label) in Ledger::groups(repo.ledger.conn())? {
        if repo
            .read_payload_string(repo.repo_scope(), &label)?
            .as_deref()
            == Some(name)
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub fn group_list(repo: &Repository) -> Result<Value> {
    let mut out = Vec::new();
    for (id, label) in Ledger::groups(repo.ledger.conn())? {
        let name = repo
            .read_payload_string(repo.repo_scope(), &label)?
            .unwrap_or_default();
        let members: Vec<Value> = Ledger::group_members(repo.ledger.conn(), id)?
            .into_iter()
            .map(|(m, kind)| json!({ "kind": kind, "id": SectionId::from_slice(&m).map(|s| s.to_string()).unwrap_or_default() }))
            .collect();
        out.push(json!({ "group": id.to_string(), "name": name, "members": members }));
    }
    Ok(json!(out))
}

pub fn group_member(
    repo: &mut Repository,
    ref_name: &str,
    name: &str,
    spec: &str,
    add: bool,
    actor: &str,
) -> Result<Value> {
    let gid =
        find_group(repo, name)?.ok_or_else(|| WispError::not_found(format!("no group {name}")))?;
    let (c, section) = resolve_target(repo, ref_name, spec)?;
    let (kind, member) = match section {
        Some(s) => ("section", s.to_string()),
        None => ("concept", c.concept.to_string()),
    };
    let op = PlannedOp::new(
        if add { Op::GroupAdd } else { Op::GroupRemove },
        Target::Group { group: gid },
        repo.repo_scope(),
    )
    .with_payload(PayloadKind::GroupMember, encode_group_member(kind, &member))
    .with_path(format!("{}.md", c.path));
    let mut req = CommitRequest::new(ref_name, actor);
    req.ops = vec![op];
    let out = commit(repo, req)?;
    Ok(
        json!({ "changeset": out.changeset.to_string(), "group": name, "member": member, "kind": kind, "added": add }),
    )
}

// ---- erase ----

#[derive(Clone, Debug)]
pub enum EraseTarget {
    Path(String),
    Group(String),
    Actor(String),
}

/// Resolve an erase target to its scope set (§10.4) and list what it covers.
pub fn resolve_scopes(
    repo: &mut Repository,
    ref_name: &str,
    target: &EraseTarget,
) -> Result<(Vec<ScopeId>, Target, Vec<Value>)> {
    let mut scopes: BTreeSet<ScopeId> = BTreeSet::new();
    let mut covers: Vec<Value> = Vec::new();
    fn add_concept(
        repo: &Repository,
        scopes: &mut BTreeSet<ScopeId>,
        covers: &mut Vec<Value>,
        cid: ConceptId,
        path: &str,
    ) -> Result<()> {
        for s in Ledger::scopes_of_concept(repo.ledger.conn(), cid)? {
            if s.erased {
                continue;
            }
            scopes.insert(s.id);
            match s.kind {
                ScopeKind::Concept { .. } => covers.push(json!({ "kind": "concept", "path": path, "scope": s.id.to_string() })),
                ScopeKind::Section { section, .. } => covers.push(json!({ "kind": "section", "path": path, "section": section.to_string(), "scope": s.id.to_string() })),
                _ => {}
            }
        }
        Ok(())
    }
    let tgt = match target {
        EraseTarget::Path(spec) => {
            if let Some((p, _)) = spec.split_once('#') {
                let (c, section) = resolve_target(repo, ref_name, spec)?;
                let s = section.expect("section spec");
                let row = Ledger::scope_of_section(repo.ledger.conn(), s)?
                    .ok_or_else(|| WispError::not_found("section scope"))?;
                scopes.insert(row.id);
                covers.push(json!({ "kind": "section", "path": c.path, "section": s.to_string(), "scope": row.id.to_string() }));
                let _ = p;
                Target::Section {
                    concept: c.concept,
                    section: s,
                }
            } else {
                // A concept path, or an asset path.
                let path = normalize_path(spec);
                let asset = {
                    repo.hot(ref_name)?;
                    Hot::asset_by_path(repo.hot(ref_name)?.conn(), &path)?
                };
                if let Some(a) = asset {
                    scopes.insert(a.scope);
                    covers.push(
                        json!({ "kind": "asset", "path": a.path, "scope": a.scope.to_string() }),
                    );
                    Target::Asset { asset: a.asset }
                } else {
                    let (c, _) = resolve_target(repo, ref_name, spec)?;
                    add_concept(repo, &mut scopes, &mut covers, c.concept, &c.path)?;
                    Target::Concept { concept: c.concept }
                }
            }
        }
        EraseTarget::Group(name) => {
            let gid = find_group(repo, name)?
                .ok_or_else(|| WispError::not_found(format!("no group {name}")))?;
            for (m, kind) in Ledger::group_members(repo.ledger.conn(), gid)? {
                match kind.as_str() {
                    "concept" => {
                        if let Some(cid) = ConceptId::from_slice(&m) {
                            let path = {
                                repo.hot(ref_name)?;
                                Hot::concept(repo.hot(ref_name)?.conn(), cid)?
                                    .map(|c| c.path)
                                    .unwrap_or_else(|| cid.to_string())
                            };
                            add_concept(repo, &mut scopes, &mut covers, cid, &path)?;
                        }
                    }
                    "section" => {
                        if let Some(sid) = SectionId::from_slice(&m) {
                            if let Some(row) = Ledger::scope_of_section(repo.ledger.conn(), sid)? {
                                if !row.erased {
                                    scopes.insert(row.id);
                                    covers.push(json!({ "kind": "section", "section": sid.to_string(), "scope": row.id.to_string() }));
                                }
                            }
                        }
                    }
                    "asset" => {
                        if let Some(aid) = AssetId::from_slice(&m) {
                            if let Some(row) = Ledger::scope_of_asset(repo.ledger.conn(), aid)? {
                                scopes.insert(row.id);
                                covers.push(json!({ "kind": "asset", "asset": aid.to_string(), "scope": row.id.to_string() }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Target::Group { group: gid }
        }
        EraseTarget::Actor(name) => {
            let row = repo
                .find_actor(name)?
                .ok_or_else(|| WispError::not_found(format!("no actor {name}")))?;
            scopes.insert(row.scope);
            covers.push(json!({ "kind": "actor", "actor": row.id.to_string(), "scope": row.scope.to_string() }));
            Target::Actor { actor: row.id }
        }
    };
    Ok((scopes.into_iter().collect(), tgt, covers))
}

/// Dependents of the erased scopes on every ref (erasure removes the key for every branch).
fn erase_dependents(repo: &Repository, scopes: &[ScopeId]) -> Result<crate::impact::AllRefs> {
    crate::impact::dependents_all_refs(repo, scopes, crate::impact::DEFAULT_DEPTH)
}

/// `wisp erase` (§10.5): commit, hot purge, cold shred, notify.
pub fn erase(
    repo: &mut Repository,
    ref_name: &str,
    target: &EraseTarget,
    reason: Reason,
    yes: bool,
    discard_unrecorded: bool,
    actor: &str,
) -> Result<Value> {
    let (scopes, tgt, covers) = resolve_scopes(repo, ref_name, target)?;
    if scopes.is_empty() {
        return Err(WispError::not_found("nothing to erase (already erased?)"));
    }
    let dependents = erase_dependents(repo, &scopes)?;
    if !yes {
        return Ok(json!({
            "dry_run": true, "would_erase": covers, "scopes": scopes.len(), "hint": "pass --yes to erase",
            "flagged": dependents.concepts.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            "dependents_by_ref": dependents.by_ref,
            "dependents_truncated": dependents.truncated,
        }));
    }
    preflight_managed_worktrees_for_erase(repo, discard_unrecorded)?;
    let markers = markers_for_scopes_on_ref(repo, ref_name, &scopes)?;
    let flagged = dependents.concepts.clone();
    let affected_paths: Vec<String> = covers
        .iter()
        .filter_map(|c| {
            c.get("path")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    // Commit step: one changeset with one erase change, plus the tombstone and scope flags, in one ledger transaction.
    let mut op = PlannedOp::new(
        Op::Erase,
        Target::Erase {
            scopes: scopes.clone(),
        },
        repo.repo_scope(),
    );
    op.reason = Some(reason);
    op.path = affected_paths.first().cloned();
    let mut req = CommitRequest::new(ref_name, actor);
    req.ops = vec![op];
    let out = commit(repo, req)?;
    let erase_change = out
        .changes
        .iter()
        .find(|c| c.op == Op::Erase)
        .map(|c| c.id)
        .expect("erase change");
    apply_erase_bookkeeping(
        repo,
        erase_change,
        &tgt,
        &scopes,
        reason,
        &flagged,
        &markers,
    )?;

    // Drain staged cold payloads before shred so async flush cannot resurrect ciphertext.
    repo.flush_cold_payloads()?;
    // Phase 1 and Phase 2 (idempotent; recovery reruns them).
    shred(repo, &scopes, erase_change, &flagged)?;
    finish_erase_worktree_rewrite(repo, discard_unrecorded)?;
    assert_erase_complete(repo, &scopes)?;
    repo.ledger.checkpoint()?;
    repo.keystore.checkpoint()?;

    let pending = Ledger::pending_erasures(repo.ledger.conn())?
        .into_iter()
        .filter(|(c, _)| *c == erase_change)
        .map(|(_, s)| s)
        .collect::<Vec<_>>();
    let mut result = json!({
        "changeset": out.changeset.to_string(),
        "erase_change": erase_change.to_string(),
        "erased": covers,
        "scopes": scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "flagged": flagged.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
        "dependents_by_ref": dependents.by_ref,
        "dependents_truncated": dependents.truncated,
        "pending_subscribers": pending,
    });
    if let Target::Actor { .. } = tgt {
        result["note"] = json!("actor identity erased; managed frontmatter regenerated");
    }
    Ok(result)
}

/// Phase 1 (hot purge across every hot tier) and Phase 2 (cold shred). Idempotent.
pub fn shred(
    repo: &mut Repository,
    scopes: &[ScopeId],
    cause: ChangeId,
    flagged: &[ConceptId],
) -> Result<()> {
    // Phase 1: every hot tier on disk.
    let names = repo.open_all_hots()?;
    let mut ledger_sections = Vec::new();
    for s in scopes {
        if let Some(row) = Ledger::scope(repo.ledger.conn(), *s)? {
            if let ScopeKind::Section { section, .. } = row.kind {
                ledger_sections.push(section);
            }
        }
    }
    let mut vector_sections = ledger_sections.clone();
    for name in &names {
        if let Some(hot) = repo.hots.get(name) {
            for s in scopes {
                vector_sections.extend(Hot::section_ids_in_scope(hot.conn(), *s)?);
            }
        }
    }
    vector_sections.sort();
    vector_sections.dedup();
    crate::vector::purge_vectors_for_sections(repo, &vector_sections)?;
    for name in &names {
        let hot = repo.hots.get_mut(name).expect("opened");
        let tx = hot.begin()?;
        for section in &ledger_sections {
            Hot::delete_section(&tx, *section)?;
        }
        for s in scopes {
            let affected = Hot::purge_scope(&tx, *s)?;
            for c in affected {
                // Links to an erased concept become unresolved with no target path.
                if Hot::concept(&tx, c)?.is_none() {
                    Hot::unresolve_links_to(&tx, c)?;
                }
            }
        }
        for c in flagged {
            Hot::add_flag(&tx, *c, cause, "source_erased")?;
        }
        // Erasure markers for sections whose concept survives.
        for (section, concept, after, reason) in Ledger::erasure_markers(repo.ledger.conn())? {
            if Hot::concept(&tx, concept)?.is_some() {
                Hot::add_erased_marker(&tx, concept, section, after, reason.as_str())?;
            }
        }
        tx.commit()
            .map_err(|e| WispError::internal(e.to_string()))?;
        hot.vacuum()?;
    }
    // Vector rows for erased sections were removed above; token DBs follow the same subscriber pattern.
    // Phase 2: cold shred.
    for s in scopes {
        repo.ledger.shred_scope_payloads(*s)?;
        repo.keystore.delete(*s)?;
        Ledger::mark_scope_shredded(repo.ledger.conn(), *s)?;
    }
    Ok(())
}

fn erase_cause_for_pending(conn: &rusqlite::Connection, pending: &[ScopeId]) -> Result<ChangeId> {
    let mut best: Option<(i64, ChangeId)> = None;
    for t in Ledger::tombstones(conn)? {
        if !t.scopes.iter().any(|s| pending.contains(s)) {
            continue;
        }
        let seq = Ledger::get_change(conn, t.erase_change)?
            .and_then(|ch| Ledger::get_changeset(conn, ch.changeset).ok().flatten())
            .map(|row| row.seq)
            .unwrap_or(0);
        if best.as_ref().map(|(s, _)| seq > *s).unwrap_or(true) {
            best = Some((seq, t.erase_change));
        }
    }
    if let Some((_, id)) = best {
        return Ok(id);
    }
    if let Some(ch) = Ledger::erase_changes_without_tombstones(conn)?.last() {
        return Ok(ch.id);
    }
    Ok(ChangeId::new())
}

/// On open: finish any interrupted erase before anything else (§10.5 Recovery).
pub fn recover(repo: &mut Repository) -> Result<()> {
    finish_erase_bookkeeping(repo)?;
    let pending = Ledger::scopes_erased_not_shredded(repo.ledger.conn())?;
    let rewrite_pending = Ledger::erase_rewrite_pending(repo.ledger.conn())?;
    if pending.is_empty() && !rewrite_pending {
        return Ok(());
    }
    if !pending.is_empty() {
        repo.flush_cold_payloads()?;
        let cause = erase_cause_for_pending(repo.ledger.conn(), &pending)?;
        shred(repo, &pending, cause, &[])?;
    }
    if rewrite_pending || !pending.is_empty() {
        finish_erase_worktree_rewrite(repo, true)?;
    }
    if !pending.is_empty() {
        assert_erase_complete(repo, &pending)?;
    }
    repo.ledger.checkpoint()?;
    repo.keystore.checkpoint()?;
    Ok(())
}

fn finish_erase_worktree_rewrite(repo: &mut Repository, discard_unrecorded: bool) -> Result<()> {
    rewrite_all_managed_worktrees(repo, discard_unrecorded)?;
    Ledger::set_erase_rewrite_pending(repo.ledger.conn(), false)?;
    Ok(())
}

/// Re-render every Wisp-managed checkout (primary root and each registered linked worktree).
pub fn rewrite_all_managed_worktrees(
    repo: &mut Repository,
    discard_unrecorded: bool,
) -> Result<()> {
    let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;
    let primary_ref = query::current_ref(repo);
    let primary_root = repo.root.clone();
    rewrite_worktree_at(repo, &primary_ref, &primary_root, discard_unrecorded)?;
    for wt in Ledger::list_worktrees(repo.ledger.conn())? {
        let path = PathBuf::from(&wt.path);
        if path.is_dir() {
            rewrite_worktree_at(repo, &wt.ref_name, &path, discard_unrecorded)?;
        }
    }
    Ok(())
}

/// Re-render checkouts that display `ref_name` (primary when checked out there, plus linked worktrees on that ref).
pub fn rewrite_worktrees_for_ref(
    repo: &mut Repository,
    ref_name: &str,
    discard_unrecorded: bool,
) -> Result<()> {
    let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;
    if query::current_ref(repo) == ref_name {
        let primary_root = repo.root.clone();
        rewrite_worktree_at(repo, ref_name, &primary_root, discard_unrecorded)?;
    }
    for wt in Ledger::list_worktrees(repo.ledger.conn())? {
        if wt.ref_name != ref_name {
            continue;
        }
        let path = PathBuf::from(&wt.path);
        if path.is_dir() {
            rewrite_worktree_at(repo, ref_name, &path, discard_unrecorded)?;
        }
    }
    Ok(())
}

fn preflight_managed_worktrees_for_erase(
    repo: &Repository,
    discard_unrecorded: bool,
) -> Result<()> {
    if discard_unrecorded {
        return Ok(());
    }
    let primary_ref = query::current_ref(repo);
    let primary_root = repo.root.clone();
    ensure_clean_for_erase_rewrite(repo, &primary_ref, &primary_root, false)?;
    for wt in Ledger::list_worktrees(repo.ledger.conn())? {
        let path = PathBuf::from(&wt.path);
        if path.is_dir() {
            ensure_clean_for_erase_rewrite(repo, &wt.ref_name, &path, false)?;
        }
    }
    Ok(())
}

fn ensure_clean_for_erase_rewrite(
    repo: &Repository,
    ref_name: &str,
    dir: &Path,
    discard_unrecorded: bool,
) -> Result<()> {
    if discard_unrecorded {
        return Ok(());
    }
    let st = crate::record::unrecorded_edits_at(repo, ref_name, dir)?;
    if st.changed.is_empty()
        && st.new.is_empty()
        && st.missing.is_empty()
        && st.generated_modified.is_empty()
    {
        return Ok(());
    }
    Err(WispError::conflict(
        "unrecorded edits in managed checkout; pass --discard to proceed with erase",
        json!([{
            "type": "unrecorded_edits",
            "changed": st.changed,
            "new": st.new,
            "missing": st.missing,
            "generated_modified": st.generated_modified,
        }]),
    ))
}

fn rewrite_worktree_at(
    repo: &mut Repository,
    ref_name: &str,
    dir: &Path,
    discard_unrecorded: bool,
) -> Result<()> {
    if Ledger::head(repo.ledger.conn(), ref_name)?.is_none() {
        return Ok(());
    }
    ensure_clean_for_erase_rewrite(repo, ref_name, dir, discard_unrecorded)?;
    let idx_path = checkout::checkout_idx_path(repo, dir);
    let prior = checkout::CheckoutIndex::load(&idx_path)?;
    let idx = checkout::write_tree(repo, ref_name, dir, false, false, true)?;
    checkout::prune_tracked_files_removed_from_index(dir, prior.as_ref(), &idx)?;
    idx.save(&idx_path)?;
    Ok(())
}

/// Erase changesets committed without a tombstone row (crash between commit and bookkeeping).
pub fn finish_erase_bookkeeping(repo: &mut Repository) -> Result<()> {
    for change in Ledger::erase_changes_without_tombstones(repo.ledger.conn())? {
        let scopes = match &change.target {
            Target::Erase { scopes } if !scopes.is_empty() => scopes.clone(),
            _ => continue,
        };
        let reason = change.reason.unwrap_or(Reason::Other);
        let flagged = erase_dependents(repo, &scopes)?.concepts;
        let markers = markers_for_scopes_any_hot(repo, &scopes)?;
        apply_erase_bookkeeping(
            repo,
            change.id,
            &change.target,
            &scopes,
            reason,
            &flagged,
            &markers,
        )?;
    }
    Ok(())
}

fn markers_for_scopes_on_ref(
    repo: &mut Repository,
    ref_name: &str,
    scopes: &[ScopeId],
) -> Result<Vec<(SectionId, ConceptId, Option<SectionId>)>> {
    let mut scope_rows = Vec::new();
    for s in scopes {
        if let Some(row) = Ledger::scope(repo.ledger.conn(), *s)? {
            scope_rows.push(row);
        }
    }
    let conn = repo.hot(ref_name)?.conn();
    let mut markers = Vec::new();
    for row in scope_rows {
        if let ScopeKind::Section { concept, section } = row.kind {
            let secs = Hot::sections_of(conn, concept)?;
            let after = section_after(&secs, section);
            markers.push((section, concept, after));
        }
    }
    Ok(markers)
}

fn markers_for_scopes_any_hot(
    repo: &mut Repository,
    scopes: &[ScopeId],
) -> Result<Vec<(SectionId, ConceptId, Option<SectionId>)>> {
    let _ = repo.open_all_hots()?;
    let mut markers = Vec::new();
    for s in scopes {
        let row = Ledger::scope(repo.ledger.conn(), *s)?;
        if let Some(row) = row {
            if let ScopeKind::Section { concept, section } = row.kind {
                for hot in repo.hots.values() {
                    let secs = Hot::sections_of(hot.conn(), concept)?;
                    if secs.iter().any(|x| x.section == section) {
                        markers.push((section, concept, section_after(&secs, section)));
                        break;
                    }
                }
            }
        }
    }
    Ok(markers)
}

fn section_after(secs: &[wisp_store::SectionState], section: SectionId) -> Option<SectionId> {
    let pos = secs.iter().position(|x| x.section == section);
    pos.and_then(|i| {
        if i == 0 {
            None
        } else {
            secs.get(i - 1).map(|x| x.section)
        }
    })
}

fn apply_erase_bookkeeping(
    repo: &mut Repository,
    erase_change: ChangeId,
    tgt: &Target,
    scopes: &[ScopeId],
    reason: Reason,
    flagged: &[ConceptId],
    markers: &[(SectionId, ConceptId, Option<SectionId>)],
) -> Result<()> {
    let tx = repo.ledger.begin_immediate()?;
    Ledger::insert_tombstone(
        &tx,
        &Tombstone {
            erase_change,
            target: tgt.clone(),
            scopes: scopes.to_vec(),
            reason,
            flagged: flagged.to_vec(),
        },
    )?;
    for s in scopes {
        Ledger::mark_scope_erased(&tx, *s)?;
        Ledger::set_scope_payloads_absent(&tx, *s)?;
    }
    for (section, concept, after) in markers {
        Ledger::insert_erasure_marker(&tx, *section, *concept, *after, reason)?;
    }
    if let Target::Actor { actor } = tgt {
        Ledger::clear_actor_index(&tx, *actor)?;
    }
    for sub in repo
        .manifest
        .subscribers
        .iter()
        .filter(|s| s.command.is_some())
    {
        Ledger::add_pending_erasure(&tx, erase_change, &sub.name)?;
    }
    Ledger::set_erase_rewrite_pending(&tx, true)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn assert_erase_complete(repo: &Repository, scopes: &[ScopeId]) -> Result<()> {
    if !Ledger::scopes_erased_not_shredded(repo.ledger.conn())?.is_empty() {
        return Err(WispError::internal("erase incomplete: pending shred"));
    }
    for s in scopes {
        let row = Ledger::scope(repo.ledger.conn(), *s)?
            .ok_or_else(|| WispError::integrity(format!("missing scope {s}")))?;
        if !row.shredded {
            return Err(WispError::internal(format!(
                "erase incomplete: scope {s} not shredded"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod erase_cause_tests {
    use super::erase_cause_for_pending;
    use crate::commit::{CommitRequest, PlannedOp};
    use crate::repo::{Repository, Unlock, MAIN_REF};
    use wisp_model::{ChangeId, Op, Reason, ScopeId, Target};
    use wisp_store::Ledger;

    fn write(root: &std::path::Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn erase_change_id(repo: &mut Repository, scope: ScopeId) -> ChangeId {
        let mut op = PlannedOp::new(
            Op::Erase,
            Target::Erase {
                scopes: vec![scope],
            },
            repo.repo_scope(),
        );
        op.reason = Some(Reason::Policy);
        let mut req = CommitRequest::new(MAIN_REF, "human:test");
        req.ops = vec![op];
        let out = crate::commit::commit(repo, req).unwrap();
        out.changes
            .iter()
            .find(|c| c.op == Op::Erase)
            .map(|c| c.id)
            .expect("erase change")
    }

    #[test]
    fn erase_cause_for_pending_picks_latest_erase_changeset() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundle");
        write(
            &root,
            "doc.md",
            "---\ntype: Doc\n---\n## A\n\none\n\n## B\n\ntwo\n",
        );
        Repository::init(&root, "0.2", &Unlock::default()).unwrap();
        let mut repo = Repository::open(&root, &Unlock::default()).unwrap();
        crate::record::record(
            &mut repo,
            &crate::record::RecordOptions {
                actor: Some("human:test".into()),
                message: Some("init".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(root.join("doc.md")).unwrap();
        let parsed = wisp_parse::parse_file(&text);
        let scope_ids: Vec<ScopeId> = parsed
            .sections()
            .iter()
            .skip(1)
            .map(|s| match &s.anchor {
                Some(wisp_parse::Anchor::Valid(id)) => {
                    Ledger::scope_of_section(repo.ledger.conn(), *id)
                        .unwrap()
                        .unwrap()
                        .id
                }
                _ => panic!("anchor"),
            })
            .collect();
        assert_eq!(scope_ids.len(), 2);
        let older = erase_change_id(&mut repo, scope_ids[0]);
        let newer = erase_change_id(&mut repo, scope_ids[1]);
        let seq = |ch: ChangeId| -> i64 {
            let row = Ledger::get_change(repo.ledger.conn(), ch).unwrap().unwrap();
            Ledger::get_changeset(repo.ledger.conn(), row.changeset)
                .unwrap()
                .unwrap()
                .seq
        };
        assert!(
            seq(newer) > seq(older),
            "second erase must land on a later ledger seq"
        );
        let pending = vec![scope_ids[0], scope_ids[1]];
        let picked = erase_cause_for_pending(repo.ledger.conn(), &pending).unwrap();
        assert_eq!(picked, newer);
    }
}

//! Trusted local-path replica sync (§11.6 subset): copy cold-tier log + payloads.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use wisp_engine::apply;
use wisp_engine::repo::{Repository, Unlock};
use wisp_model::*;
use wisp_store::{Ledger, Tombstone};

/// Parse `file:///path`, `file://localhost/path`, or a plain filesystem path.
pub fn parse_local_url(url: &str) -> Result<PathBuf> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("file://") {
        let path = if rest.starts_with('/') {
            PathBuf::from(rest)
        } else if let Some(p) = rest.strip_prefix("localhost") {
            PathBuf::from(p.trim_start_matches('/'))
        } else {
            PathBuf::from(rest)
        };
        return path.canonicalize().map_err(|e| {
            WispError::not_found(format!("remote path {}: {e}", path.display()))
        });
    }
    PathBuf::from(url)
        .canonicalize()
        .map_err(|e| WispError::not_found(format!("remote path {url}: {e}")))
}

/// Empty replica with the same repo identity and keys as `template` (trusted peer).
pub fn init_trusted_replica(
    dst_root: &Path,
    template: &Repository,
    unlock: &Unlock,
) -> Result<Repository> {
    let mut dst = Repository::init(dst_root, template.profile.okf_version(), unlock)?;
    dst.manifest.repo.id = template.manifest.repo.id;
    dst.manifest.repo.scope = template.manifest.repo.scope;
    dst.manifest.save(&dst.wisp_dir.join("manifest.toml"))?;
    let keys = template.wisp_dir.join("keys");
    let dst_keys = dst.wisp_dir.join("keys");
    fs::remove_dir_all(&dst_keys)?;
    copy_dir_all(&keys, &dst_keys)?;
    Repository::open(dst_root, unlock)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let to = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir_all(&p, &to)?;
        } else {
            fs::copy(&p, &to)?;
        }
    }
    Ok(())
}

pub fn push(
    local: &mut Repository,
    remote_root: &Path,
    ref_name: &str,
    unlock: &Unlock,
) -> Result<Value> {
    let mut remote = Repository::open(remote_root, unlock)?;
    ensure_same_repo(local, &remote)?;
    let stats = transfer_cold(local, &mut remote)?;
    let head = Ledger::head(local.ledger.conn(), ref_name)?
        .ok_or_else(|| WispError::usage(format!("ref {ref_name} has no head")))?;
    let updated = push_ref_fast_forward(remote.ledger.conn(), ref_name, head)?;
    if updated {
        apply::rebuild_hot(&mut remote, ref_name)?;
    }
    Ok(json!({
        "direction": "push",
        "ref": ref_name,
        "head": head.to_string(),
        "ref_updated": updated,
        "changesets": stats.changesets,
        "payloads": stats.payloads,
        "tombstones": stats.tombstones,
    }))
}

pub fn pull(
    local: &mut Repository,
    remote_root: &Path,
    ref_name: &str,
    unlock: &Unlock,
) -> Result<Value> {
    let remote = Repository::open(remote_root, unlock)?;
    ensure_same_repo(local, &remote)?;
    let stats = transfer_cold(&remote, local)?;
    let head = Ledger::head(remote.ledger.conn(), ref_name)?
        .ok_or_else(|| WispError::usage(format!("ref {ref_name} has no head on remote")))?;
    let updated = push_ref_fast_forward(local.ledger.conn(), ref_name, head)?;
    if updated {
        apply::rebuild_hot(local, ref_name)?;
    }
    Ok(json!({
        "direction": "pull",
        "ref": ref_name,
        "head": head.to_string(),
        "ref_updated": updated,
        "changesets": stats.changesets,
        "payloads": stats.payloads,
        "tombstones": stats.tombstones,
    }))
}

fn ensure_same_repo(a: &Repository, b: &Repository) -> Result<()> {
    if a.repo_id() != b.repo_id() {
        return Err(WispError::usage(format!(
            "repo id mismatch (local {}, remote {}); trusted replicas must share identity",
            a.repo_id(),
            b.repo_id()
        )));
    }
    Ok(())
}

struct TransferStats {
    changesets: usize,
    payloads: usize,
    tombstones: usize,
}

fn transfer_cold(src: &Repository, dst: &mut Repository) -> Result<TransferStats> {
    let src_conn = src.ledger.conn();
    let mut stats = TransferStats {
        changesets: 0,
        payloads: 0,
        tombstones: 0,
    };

    for t in Ledger::tombstones(src_conn)? {
        if !tombstone_present(dst.ledger.conn(), &t)? {
            Ledger::insert_tombstone(dst.ledger.conn(), &t)?;
            stats.tombstones += 1;
        }
    }

    for a in Ledger::actors(src_conn)? {
        if Ledger::actor(dst.ledger.conn(), a.id)?.is_none() {
            stats.payloads += copy_payload(src, dst, &a.name, a.scope)?;
            if let Some(pk) = &a.public_key {
                stats.payloads += copy_payload(src, dst, pk, a.scope)?;
            }
            Ledger::insert_actor(dst.ledger.conn(), &a)?;
        }
    }

    for s in Ledger::all_scopes(src_conn)? {
        if Ledger::scope(dst.ledger.conn(), s.id)?.is_none() {
            Ledger::insert_scope(dst.ledger.conn(), s.id, &s.kind)?;
            if s.erased {
                Ledger::mark_scope_erased(dst.ledger.conn(), s.id)?;
            }
            if s.shredded {
                Ledger::mark_scope_shredded(dst.ledger.conn(), s.id)?;
            }
        }
    }

    for (gid, label) in Ledger::groups(src_conn)? {
        let members = Ledger::group_members(src_conn, gid)?;
        if Ledger::groups(dst.ledger.conn())?.iter().all(|(g, _)| *g != gid) {
            stats.payloads += copy_payload(src, dst, &label, src.repo_scope())?;
            Ledger::insert_group(dst.ledger.conn(), gid, &label)?;
            for (m, kind) in members {
                Ledger::group_add_member(dst.ledger.conn(), gid, &m, &kind)?;
            }
        }
    }

    for (section, concept, after, reason) in Ledger::erasure_markers(src_conn)? {
        let existing = Ledger::erasure_markers(dst.ledger.conn())?;
        if !existing.iter().any(|(s, _, _, _)| *s == section) {
            Ledger::insert_erasure_marker(dst.ledger.conn(), section, concept, after, reason)?;
        }
    }

    for row in Ledger::changesets(src_conn)? {
        if Ledger::get_changeset(dst.ledger.conn(), row.changeset.id)?.is_some() {
            continue;
        }
        for p in &row.changeset.parents {
            if Ledger::get_changeset(dst.ledger.conn(), *p)?.is_none() {
                return Err(WispError::integrity(format!(
                    "missing parent {} when transferring {}",
                    p,
                    row.changeset.id
                )));
            }
        }
        if let Some(b) = row.changeset.based_on {
            if Ledger::get_changeset(dst.ledger.conn(), b)?.is_none() {
                return Err(WispError::integrity(format!(
                    "missing based_on {} when transferring {}",
                    b,
                    row.changeset.id
                )));
            }
        }
        if let Some(m) = &row.changeset.message {
            stats.payloads += copy_payload(src, dst, m, src.repo_scope())?;
        }
        let changes = Ledger::changes_of(src_conn, row.changeset.id)?;
        for c in &changes {
            stats.payloads += copy_payload_for_change(src, dst, c)?;
        }
        Ledger::insert_changeset(dst.ledger.conn(), &row.changeset, &changes)?;
        stats.changesets += 1;
    }

    dst.ledger.flush_staged()?;
    Ok(stats)
}

fn tombstone_present(conn: &rusqlite::Connection, t: &Tombstone) -> Result<bool> {
    Ok(Ledger::tombstones(conn)?
        .iter()
        .any(|x| x.erase_change == t.erase_change))
}

fn copy_payload_for_change(src: &Repository, dst: &mut Repository, c: &Change) -> Result<usize> {
    let mut n = 0;
    if let Some(p) = &c.payload {
        n += copy_payload(src, dst, p, c.scope)?;
    }
    if let Some(p) = &c.note {
        n += copy_payload(src, dst, p, c.scope)?;
    }
    Ok(n)
}

fn copy_payload(
    src: &Repository,
    dst: &mut Repository,
    pr: &PayloadRef,
    _scope: ScopeId,
) -> Result<usize> {
    if Ledger::payload_present(dst.ledger.conn(), pr.id)? {
        return Ok(0);
    }
    let Some(ct) = Ledger::read_payload_at(
        src.ledger.conn(),
        &src.ledger.payloads_dir(),
        pr.id,
    )?
    else {
        return Ok(0);
    };
    let (meta_scope, kind, size) = Ledger::payload_info(src.ledger.conn(), pr.id)?
        .ok_or_else(|| WispError::integrity(format!("payload {} missing on source", pr.id)))?;
    Ledger::insert_payload_row(dst.ledger.conn(), pr.id, meta_scope, kind, size)?;
    Ledger::stage_payload(dst.ledger.conn_mut(), pr.id, &ct)?;
    Ok(1)
}

fn push_ref_fast_forward(
    conn: &rusqlite::Connection,
    ref_name: &str,
    new_head: ChangesetId,
) -> Result<bool> {
    match Ledger::head(conn, ref_name)? {
        None => {
            Ledger::set_head(conn, ref_name, new_head)?;
            Ok(true)
        }
        Some(h) if h == new_head => Ok(false),
        Some(h) => {
            if is_ancestor(conn, h, new_head)? {
                Ledger::set_head(conn, ref_name, new_head)?;
                Ok(true)
            } else {
                Err(WispError::usage(
                    "non-fast-forward ref update (merge on remote first)",
                ))
            }
        }
    }
}

fn is_ancestor(
    conn: &rusqlite::Connection,
    ancestor: ChangesetId,
    desc: ChangesetId,
) -> Result<bool> {
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


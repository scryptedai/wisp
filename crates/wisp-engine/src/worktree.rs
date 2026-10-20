//! Linked worktrees (§11.4): separate checkouts under `.wisp/worktrees/` with their own `checkout.idx`.

use crate::checkout::{self, WorktreeLock};
use crate::query;
use crate::repo::Repository;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use wisp_model::*;
use wisp_store::Ledger;

const PRIMARY_NAME: &str = "main";

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == PRIMARY_NAME {
        return Err(WispError::usage(format!("invalid worktree name: {name}")));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(WispError::usage(format!("invalid worktree name: {name}")));
    }
    Ok(())
}

fn resolve_path(repo: &Repository, name: &str, path: Option<&Path>) -> Result<PathBuf> {
    let p = match path {
        Some(p) => {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                repo.root.join(p)
            }
        }
        None => repo.wisp_dir.join("worktrees").join(name),
    };
    let canonical = std::fs::canonicalize(&p).or_else(|_| {
        std::fs::create_dir_all(&p)?;
        std::fs::canonicalize(&p)
    })?;
    let root = std::fs::canonicalize(&repo.root)?;
    if !canonical.starts_with(&root) {
        return Err(WispError::usage(
            "worktree path must be inside the repository",
        ));
    }
    let wisp = std::fs::canonicalize(&repo.wisp_dir)?;
    if canonical.starts_with(&wisp) && canonical != wisp {
        let under_worktrees = wisp.join("worktrees");
        if !canonical.starts_with(&under_worktrees) {
            return Err(WispError::usage(
                "worktree path under .wisp/ must be in .wisp/worktrees/",
            ));
        }
    }
    Ok(canonical)
}

fn ref_checked_out_elsewhere(repo: &Repository, ref_name: &str) -> Result<()> {
    if query::current_ref(repo) == ref_name {
        return Err(WispError::usage(format!(
            "ref {ref_name} is checked out in the primary worktree"
        )));
    }
    if let Some(wt) = Ledger::worktree_by_ref(repo.ledger.conn(), ref_name)? {
        return Err(WispError::usage(format!(
            "ref {ref_name} is already checked out in worktree {}",
            wt.name
        )));
    }
    Ok(())
}

/// `wisp worktree add <name>`: checkout a ref into a linked directory and register it.
pub fn add(
    repo: &mut Repository,
    name: &str,
    branch: Option<&str>,
    from: Option<&str>,
    path: Option<&Path>,
) -> Result<Value> {
    validate_name(name)?;
    if Ledger::get_worktree(repo.ledger.conn(), name)?.is_some() {
        return Err(WispError::usage(format!("worktree {name} already exists")));
    }
    let dir = resolve_path(repo, name, path)?;
    if dir.exists() && dir.read_dir()?.next().is_some() {
        return Err(WispError::usage(format!(
            "worktree directory {} is not empty",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir)?;

    let ref_name = if let Some(b) = branch {
        query::branch(repo, b, from)?;
        b.to_string()
    } else {
        from.map(|s| s.to_string())
            .unwrap_or_else(|| query::current_ref(repo))
    };

    if Ledger::head(repo.ledger.conn(), &ref_name)?.is_none() {
        return Err(WispError::not_found(format!("no ref {ref_name}")));
    }
    ref_checked_out_elsewhere(repo, &ref_name)?;

    let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;
    let idx = checkout::write_tree(repo, &ref_name, &dir, false, false, true)?;
    idx.save(&checkout::checkout_idx_path(repo, &dir))?;

    let path_str = dir.to_string_lossy().into_owned();
    let created_at = now_rfc3339();
    let tx = repo.ledger.begin_immediate()?;
    Ledger::insert_worktree(&tx, name, &path_str, &ref_name, &created_at)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;

    Ok(json!({
        "name": name,
        "path": path_str,
        "ref": ref_name,
        "head": idx.head.map(|h| h.to_string()),
        "created_at": created_at,
    }))
}

/// `wisp worktree list`: primary worktree plus registered linked worktrees.
pub fn list(repo: &Repository) -> Result<Value> {
    let primary_ref = query::current_ref(repo);
    let primary_head = repo.head(&primary_ref)?;
    let mut out = vec![json!({
        "name": PRIMARY_NAME,
        "path": repo.root.to_string_lossy(),
        "ref": primary_ref,
        "head": primary_head.map(|h| h.to_string()),
        "primary": true,
    })];
    for wt in Ledger::list_worktrees(repo.ledger.conn())? {
        let head = repo.head(&wt.ref_name)?;
        out.push(json!({
            "name": wt.name,
            "path": wt.path,
            "ref": wt.ref_name,
            "head": head.map(|h| h.to_string()),
            "created_at": wt.created_at,
            "primary": false,
        }));
    }
    Ok(json!({ "worktrees": out }))
}

/// `wisp worktree remove <name>`: drop registration and delete the checkout directory.
pub fn remove(repo: &mut Repository, name: &str, force: bool) -> Result<Value> {
    validate_name(name)?;
    let wt = Ledger::get_worktree(repo.ledger.conn(), name)?
        .ok_or_else(|| WispError::not_found(format!("no worktree {name}")))?;
    let dir = PathBuf::from(&wt.path);
    if dir.exists() {
        let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if !force {
                return Err(WispError::internal(format!(
                    "remove {}: {e}",
                    dir.display()
                )));
            }
        }
    }
    let tx = repo.ledger.begin_immediate()?;
    Ledger::delete_worktree(&tx, name)?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;
    Ok(json!({ "removed": name, "path": wt.path, "ref": wt.ref_name }))
}

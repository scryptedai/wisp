//! Rebuildable state under `.wisp/cache/` (hot SQLite, merge-skip JSON, embed caches).
//!
//! **Windows:** SQLite keeps files open while a [`Repository`] (or [`Hot`](wisp_store::Hot))
//! handle exists. Delete or rename under `cache/` only after [`Repository::close_all_hots`]
//! and dropping every `Repository` for that store (or use [`reopen_without_cache`]).

use crate::repo::{Repository, Unlock};
use std::path::Path;
use wisp_model::*;

/// Delete `.wisp/cache` (rebuilt from ledger on next open).
///
/// Call only when no `Repository` / `Hot` for this store holds files under `cache/`.
pub fn remove_rebuildable_cache(wisp_dir: &Path) -> Result<()> {
    let cache = wisp_dir.join("cache");
    if !cache.is_dir() {
        return Ok(());
    }
    std::fs::remove_dir_all(&cache).map_err(|e| {
        WispError::internal(format!(
            "remove {}: {e} — close all hot tiers and drop Repository handles first",
            cache.display()
        ))
    })?;
    Ok(())
}

/// Cold-start simulation: drop `repo`, remove rebuildable cache, reopen at `repo.root`.
pub fn reopen_without_cache(repo: Repository, unlock: &Unlock) -> Result<Repository> {
    let root = repo.root.clone();
    let wisp_dir = repo.wisp_dir.clone();
    drop(repo);
    remove_rebuildable_cache(&wisp_dir)?;
    Repository::open(&root, unlock)
}

//! `wisp-git`: git import and export as transports (§13.2, §13.3).

mod archive;
mod import_git;

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use wisp_engine::checkout;
use wisp_engine::repo::Repository;
use wisp_model::*;

pub use archive::export_archive;
pub use import_git::{import_git, ImportGitOptions};

/// Render `ref_name` into `dir`, `git init`, add all, one commit.
pub fn export_git(repo: &mut Repository, ref_name: &str, dir: &Path) -> Result<Value> {
    if dir.exists() {
        let mut empty = true;
        if dir.is_dir() {
            if let Ok(mut it) = std::fs::read_dir(dir) {
                empty = it.next().is_none();
            }
        } else {
            empty = false;
        }
        if !empty {
            return Err(WispError::usage(format!(
                "export --git target {} is not empty",
                dir.display()
            )));
        }
    }
    std::fs::create_dir_all(dir)?;
    let idx = checkout::checkout_with_profile(repo, ref_name, Some(dir), None, false, false)?;
    run_git(dir, &["init"])?;
    run_git(dir, &["add", "-A"])?;
    let msg = format!("wisp export {} @ {}", ref_name, repo.repo_id());
    run_git(dir, &["commit", "-m", &msg])?;
    Ok(json!({
        "git_dir": dir.display().to_string(),
        "ref": ref_name,
        "files": idx.files.len(),
        "commits": 1,
        "note": "single-commit export; per-changeset git history is not implemented"
    }))
}

fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| WispError::internal(format!("git {:?}: {e}", args)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(WispError::internal(format!(
            "git {:?} failed: {}",
            args,
            stderr.trim()
        )));
    }
    Ok(())
}

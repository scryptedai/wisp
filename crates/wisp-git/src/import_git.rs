//! Import a git repository snapshot via `git archive` (§13.3 phase A).

use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use tar::Archive;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::Repository;
use wisp_model::*;

pub struct ImportGitOptions {
    pub prefix: Option<String>,
    pub git_ref: Option<String>,
    pub squash_message: Option<String>,
}

/// Extract `git_ref` (default `HEAD`) with `git archive`, copy into `repo.root`, then `record`.
pub fn import_git(
    repo: &mut Repository,
    git_dir: &Path,
    opts: ImportGitOptions,
) -> Result<Value> {
    let git_dir = git_dir
        .canonicalize()
        .map_err(|e| WispError::not_found(format!("git repo {}: {e}", git_dir.display())))?;
    if !git_dir.join(".git").exists() && !git_dir.is_dir() {
        return Err(WispError::usage(format!(
            "{} does not look like a git repository",
            git_dir.display()
        )));
    }
    let git_ref = opts.git_ref.as_deref().unwrap_or("HEAD");
    let tmp = tempfile::tempdir().map_err(|e| WispError::internal(e.to_string()))?;
    let mut child = Command::new("git")
        .args(["-C", git_dir.to_str().unwrap(), "archive", "--format=tar", git_ref])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| WispError::internal(format!("git archive: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WispError::internal("git archive: no stdout"))?;
    let mut archive = Archive::new(stdout);
    archive
        .unpack(tmp.path())
        .map_err(|e| WispError::internal(format!("unpack git archive: {e}")))?;
    let status = child
        .wait()
        .map_err(|e| WispError::internal(e.to_string()))?;
    if !status.success() {
        return Err(WispError::internal("git archive failed"));
    }
    let mut files = 0usize;
    copy_tree(tmp.path(), opts.prefix.as_deref(), &repo.root, &mut files)?;
    let actor = repo.default_actor();
    let msg = opts
        .squash_message
        .unwrap_or_else(|| format!("import --git {}", git_dir.display()));
    let out = record(
        repo,
        &RecordOptions {
            actor: Some(actor),
            message: Some(msg),
            full: true,
            ..Default::default()
        },
    )?;
    Ok(json!({
        "git_dir": git_dir.display().to_string(),
        "git_ref": git_ref,
        "files": files,
        "changeset": out.changeset,
        "note": "snapshot via git archive; per-commit history is not mapped yet",
    }))
}

fn copy_tree(src: &Path, prefix: Option<&str>, dest_root: &Path, files: &mut usize) -> Result<()> {
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            continue;
        }
        let rel = entry.path().strip_prefix(src).unwrap();
        let rel_s = rel.to_string_lossy();
        if rel_s.starts_with(".git/") || rel_s == ".git" || rel_s.starts_with(".wisp/") {
            continue;
        }
        if !is_text_path(&rel_s) {
            continue;
        }
        let dest_rel = match prefix {
            Some(p) => format!("{}/{}", p.trim_end_matches('/'), rel_s),
            None => rel_s.to_string(),
        };
        let to = dest_root.join(&dest_rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), &to)?;
        *files += 1;
    }
    Ok(())
}

fn is_text_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
        || lower.ends_with(".py")
        || lower.ends_with(".rs")
        || lower.ends_with(".toml")
        || !lower.contains('.')
}

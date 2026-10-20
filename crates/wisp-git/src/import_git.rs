//! Import from git: snapshot (`git archive`) or history (`git log` + one changeset per commit).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tar::Archive;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::Repository;
use wisp_model::*;

const GIT_IMPORT_ACTOR: &str = "process:wisp-git-import/1.0";
const GIT_MAP: &str = "meta/git-map.json";

pub struct ImportGitOptions {
    pub prefix: Option<String>,
    pub git_ref: Option<String>,
    pub squash_message: Option<String>,
    /// One wisp changeset per git commit (oldest first).
    pub history: bool,
}

pub fn import_git(repo: &mut Repository, git_dir: &Path, opts: ImportGitOptions) -> Result<Value> {
    let git_dir = canonical_git_dir(git_dir)?;
    if opts.history {
        return import_git_history(repo, &git_dir, &opts);
    }
    import_git_snapshot(repo, &git_dir, &opts)
}

fn canonical_git_dir(git_dir: &Path) -> Result<PathBuf> {
    let git_dir = git_dir
        .canonicalize()
        .map_err(|e| WispError::not_found(format!("git repo {}: {e}", git_dir.display())))?;
    if !git_dir.join(".git").exists() {
        return Err(WispError::usage(format!(
            "{} does not look like a git repository",
            git_dir.display()
        )));
    }
    Ok(git_dir)
}

fn import_git_snapshot(
    repo: &mut Repository,
    git_dir: &Path,
    opts: &ImportGitOptions,
) -> Result<Value> {
    let git_ref = opts.git_ref.as_deref().unwrap_or("HEAD");
    let files = extract_commit_to_root(repo, git_dir, git_ref, opts.prefix.as_deref())?;
    let msg = opts
        .squash_message
        .clone()
        .unwrap_or_else(|| format!("import --git {}", git_dir.display()));
    let out = record_snapshot(repo, &msg)?;
    Ok(json!({
        "mode": "snapshot",
        "git_dir": git_dir.display().to_string(),
        "git_ref": git_ref,
        "files": files,
        "changeset": out.changeset,
    }))
}

fn import_git_history(
    repo: &mut Repository,
    git_dir: &Path,
    opts: &ImportGitOptions,
) -> Result<Value> {
    let range = opts.git_ref.as_deref().unwrap_or("HEAD");
    let commits = git_commits_reverse(git_dir, range)?;
    let git_commit_count = commits.len();
    let mut mapped = Vec::new();
    let mut changesets = 0usize;
    for (sha, subject) in commits {
        clear_bundle_root(&repo.root)?;
        let files = extract_commit_to_root(repo, git_dir, &sha, opts.prefix.as_deref())?;
        if files == 0 {
            continue;
        }
        let out = record_snapshot(repo, &format!("git {sha}: {subject}"))?;
        if let Some(cs) = out.changeset {
            changesets += 1;
            mapped.push(json!({
                "git": sha,
                "wisp": cs,
                "subject": subject,
            }));
        }
    }
    let map_path = repo.wisp_dir.join(GIT_MAP);
    if let Some(parent) = map_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let map = json!({
        "source": git_dir.display().to_string(),
        "range": range,
        "commits": mapped,
    });
    std::fs::write(&map_path, serde_json::to_vec_pretty(&map)?)?;
    Ok(json!({
        "mode": "history",
        "git_dir": git_dir.display().to_string(),
        "range": range,
        "git_commits": git_commit_count,
        "wisp_changesets": changesets,
        "map": map_path.display().to_string(),
    }))
}

fn record_snapshot(
    repo: &mut Repository,
    message: &str,
) -> Result<wisp_engine::record::RecordResult> {
    record(
        repo,
        &RecordOptions {
            actor: Some(GIT_IMPORT_ACTOR.into()),
            message: Some(message.into()),
            full: true,
            ..Default::default()
        },
    )
}

fn git_commits_reverse(git_dir: &Path, range: &str) -> Result<Vec<(String, String)>> {
    let out = Command::new("git")
        .args([
            "-C",
            git_dir.to_str().unwrap(),
            "log",
            "--reverse",
            "--format=%H%x00%s",
            range,
        ])
        .output()
        .map_err(|e| WispError::internal(format!("git log: {e}")))?;
    if !out.status.success() {
        return Err(WispError::internal(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let mut v = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let (sha, subj) = line
            .split_once('\0')
            .ok_or_else(|| WispError::internal("git log: bad format"))?;
        v.push((sha.to_string(), subj.to_string()));
    }
    Ok(v)
}

fn extract_commit_to_root(
    repo: &mut Repository,
    git_dir: &Path,
    git_ref: &str,
    prefix: Option<&str>,
) -> Result<usize> {
    let tmp = tempfile::tempdir().map_err(|e| WispError::internal(e.to_string()))?;
    git_archive_to_dir(git_dir, git_ref, tmp.path())?;
    let mut files = 0usize;
    copy_tree(tmp.path(), prefix, &repo.root, &mut files)?;
    Ok(files)
}

fn git_archive_to_dir(git_dir: &Path, git_ref: &str, dest: &Path) -> Result<()> {
    let mut child = Command::new("git")
        .args([
            "-C",
            git_dir.to_str().unwrap(),
            "archive",
            "--format=tar",
            git_ref,
        ])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| WispError::internal(format!("git archive: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WispError::internal("git archive: no stdout"))?;
    let mut archive = Archive::new(stdout);
    archive
        .unpack(dest)
        .map_err(|e| WispError::internal(format!("unpack git archive: {e}")))?;
    let status = child
        .wait()
        .map_err(|e| WispError::internal(e.to_string()))?;
    if !status.success() {
        return Err(WispError::internal("git archive failed"));
    }
    Ok(())
}

fn clear_bundle_root(root: &Path) -> Result<()> {
    for e in std::fs::read_dir(root)? {
        let e = e?;
        let name = e.file_name();
        if name == ".wisp" || name == ".wispignore" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, prefix: Option<&str>, dest_root: &Path, files: &mut usize) -> Result<()> {
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(|e| e.ok())
    {
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

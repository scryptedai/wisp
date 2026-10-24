//! Resolve installed topic documentation (share/doc layout).

use std::path::{Path, PathBuf};

/// Topics exposed by `wisp doc <topic>`.
pub const TOPICS: &[(&str, &str)] = &[
    ("cli", "CLI.md"),
    ("erase", "ERASE.md"),
    ("merge", "MERGE.md"),
    ("search", "SEARCH.md"),
    ("why", "WHY.md"),
    ("impact", "IMPACT.md"),
    ("apply", "APPLY.md"),
    ("trust", "TRUST-BOUNDARY.md"),
    ("concurrency", "CONCURRENCY.md"),
    ("extensions", "EXTENSIONS.md"),
    ("export-html", "EXPORT-HTML.md"),
    ("git", "GIT-INTEROP.md"),
    ("invariants", "INVARIANTS.md"),
    ("decisions", "DECISIONS.md"),
    ("agents", "AGENTS.md"),
];

/// Directory containing `wisp.1` (parent of `man1/`).
pub fn man_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WISP_MAN_ROOT") {
        let p = PathBuf::from(p);
        if p.join("man1/wisp.1").is_file() {
            return Some(p);
        }
    }
    let data = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok()?;
    let user = data.join("man");
    if user.join("man1/wisp.1").is_file() {
        return Some(user);
    }
    for p in [
        PathBuf::from("/usr/local/share/man"),
        PathBuf::from("/usr/share/man"),
    ] {
        if p.join("man1/wisp.1").is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            let share = bin.parent()?.join("share/man");
            if share.join("man1/wisp.1").is_file() {
                return Some(share);
            }
        }
    }
    None
}

pub fn man_page_path() -> Option<PathBuf> {
    if let Some(root) = man_root() {
        let p = root.join("man1/wisp.1");
        if p.is_file() {
            return Some(p);
        }
    }
    dev_doc_root_from_cwd().and_then(|d| {
        let repo = d.parent()?.to_path_buf();
        let p = repo.join("contrib/man/wisp.1");
        p.is_file().then_some(p)
    })
}

pub fn doc_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WISP_DOC_ROOT") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    let data = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok()?;
    let user = data.join("wisp/doc");
    if user.is_dir() {
        return Some(user);
    }
    for p in [
        PathBuf::from("/usr/local/share/doc/wisp"),
        PathBuf::from("/usr/share/doc/wisp"),
    ] {
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            let prefix = bin.parent()?;
            let share = prefix.join("share/doc/wisp");
            if share.is_dir() {
                return Some(share);
            }
        }
    }
    dev_doc_root_from_cwd()
}

/// When hacking the wisp tree without `install-docs.sh`, use `docs/` from the repo root.
fn dev_doc_root_from_cwd() -> Option<PathBuf> {
    let mut d = std::env::current_dir().ok()?;
    for _ in 0..12 {
        if d.join("docs/CLI.md").is_file() {
            return Some(d.join("docs"));
        }
        d = d.parent()?.to_path_buf();
    }
    None
}

pub fn topic_path(topic: &str) -> Option<(PathBuf, String)> {
    let root = doc_root()?;
    let file = TOPICS
        .iter()
        .find(|(name, _)| *name == topic)
        .map(|(_, f)| *f)
        .or_else(|| {
            if topic.ends_with(".md") {
                Some(topic)
            } else {
                None
            }
        })?;
    let path = root.join(file);
    if path.is_file() {
        return Some((path, file.to_string()));
    }
    if file == "DECISIONS.md" || file == "AGENTS.md" {
        if let Some(repo) = root.parent() {
            let alt = repo.join(file);
            if alt.is_file() {
                return Some((alt, file.to_string()));
            }
        }
    }
    None
}

pub fn list_topics(root: &Path) -> Vec<(String, PathBuf, bool)> {
    TOPICS
        .iter()
        .map(|(name, file)| {
            let path = root.join(file);
            let present = path.is_file()
                || ((*file == "DECISIONS.md" || *file == "AGENTS.md")
                    && root.parent().is_some_and(|p| p.join(file).is_file()));
            (name.to_string(), path, present)
        })
        .collect()
}

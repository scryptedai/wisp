//! Views (§13.1): rendering concepts, generated `index.md`/`log.md`, the working tree, and `checkout.idx`.

use crate::repo::Repository;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use wisp_model::*;
use wisp_profile::{ConceptView, DirEntry, DirView, LogEntry};
use wisp_store::{ConceptState, Hot, Ledger};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileRecord {
    pub size: u64,
    pub mtime_ns: i128,
    /// SHA-256 of the bytes wisp last wrote, so an mtime-only touch is not an edit.
    #[serde(default)]
    pub hash: String,
    pub kind: String, // concept | asset | generated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

pub fn hash_bytes(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

/// Has the file changed since wisp wrote it? Cheap size/mtime check first, then the content hash.
pub fn file_changed(path: &Path, rec: &FileRecord) -> bool {
    let Ok(md) = std::fs::metadata(path) else {
        return true;
    };
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    if md.len() == rec.size && mtime_ns == rec.mtime_ns {
        return false;
    }
    match std::fs::read(path) {
        Ok(b) => hash_bytes(&b) != rec.hash,
        Err(_) => true,
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConceptBase {
    pub path: String,
    pub version: ChangeId,
    #[serde(default)]
    pub content_version: Option<ChangeId>,
    pub sections: Vec<(SectionId, ChangeId)>,
    pub entries: BTreeMap<String, ChangeId>,
    pub sources: BTreeMap<String, ChangeId>,
}

/// `.wisp/checkout.idx`: base versions and change-detection records for the working tree (§9, §13.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckoutIndex {
    pub ref_name: String,
    pub head: Option<ChangesetId>,
    pub files: BTreeMap<String, FileRecord>,
    pub concepts: BTreeMap<String, ConceptBase>,
    pub assets: BTreeMap<String, (AssetId, ChangeId)>,
}

impl CheckoutIndex {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }
}

pub fn file_record(path: &Path, kind: &str, id: Option<String>) -> Result<FileRecord> {
    let md = std::fs::metadata(path)
        .map_err(|e| WispError::internal(format!("stat {}: {e}", path.display())))?;
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    let hash = std::fs::read(path)
        .map(|b| hash_bytes(&b))
        .unwrap_or_default();
    Ok(FileRecord {
        size: md.len(),
        mtime_ns,
        hash,
        kind: kind.to_string(),
        id,
    })
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{}.{}.wisp-tmp", name, ChangeId::new()));
    std::fs::write(&tmp, bytes)
        .map_err(|e| WispError::internal(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        WispError::internal(format!(
            "rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

/// Build the profile's view of a concept from the hot tier.
pub fn concept_view(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
    include_retracted: Option<(String, String, String)>,
) -> Result<ConceptView> {
    let managed: &[&str] = repo.profile.managed_keys();
    let entries: Vec<(String, String)> = Hot::frontmatter_of(hot, c.concept)?
        .into_iter()
        .filter(|e| !managed.contains(&e.key.as_str()) && e.key != repo.identity_key())
        .map(|e| (e.key, e.raw))
        .collect();
    let sections: Vec<(u8, String, String)> = Hot::sections_of(hot, c.concept)?
        .into_iter()
        .map(|s| (s.level, s.heading, s.body))
        .collect();
    let sources = Hot::sources_of(hot, c.concept)?;
    let generated = match (&c.gen_actor, &c.gen_at) {
        (Some(a), Some(at)) => Some((repo.actor_display(*a), at.clone())),
        _ => None,
    };
    let verified = match c.content_version {
        Some(cv) => Hot::verified(hot, c.concept, cv)?
            .into_iter()
            .map(|(a, at)| (repo.actor_display(a), at))
            .collect(),
        None => vec![],
    };
    let footnotes: Vec<String> = Hot::sections_of(hot, c.concept)?
        .iter()
        .flat_map(|s| {
            wisp_parse::sections::section(&s.body)
                .sections
                .into_iter()
                .flat_map(|p| p.footnotes)
        })
        .collect();
    let line_ending = detect_line_ending(hot, c.concept)?;
    Ok(ConceptView {
        path: c.path.clone(),
        wisp_id: Some(c.concept),
        type_: c.type_.clone(),
        entries,
        sections,
        footnotes,
        source_ids: sources.iter().map(|s| s.source_id.clone()).collect(),
        sources: sources.iter().map(|s| s.record.clone()).collect(),
        status: c.status,
        generated,
        verified,
        retracted: include_retracted,
        line_ending,
    })
}

fn detect_line_ending(hot: &rusqlite::Connection, concept: ConceptId) -> Result<String> {
    for s in Hot::sections_of(hot, concept)? {
        if s.body.contains("\r\n") || s.heading_line.contains("\r\n") {
            return Ok("\r\n".into());
        }
        if s.body.contains('\n') {
            return Ok("\n".into());
        }
    }
    for e in Hot::frontmatter_of(hot, concept)? {
        if e.raw.contains("\r\n") {
            return Ok("\r\n".into());
        }
    }
    Ok("\n".into())
}

/// Render a concept file: frontmatter via the profile, then the sections with anchors and erasure markers.
pub fn render_concept(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
    strip_anchors: bool,
    retracted: Option<(String, String, String)>,
) -> Result<String> {
    let view = concept_view(repo, hot, c, retracted)?;
    let le = view.line_ending.clone();
    let mut out = repo.profile.render_frontmatter(&view, repo.identity_key());
    let sections = Hot::sections_of(hot, c.concept)?;
    let markers = if repo.manifest.render.erasure_markers {
        Hot::erased_markers(hot, c.concept)?
    } else {
        vec![]
    };
    let rendered: Vec<(Option<SectionId>, u8, &str, &str)> = sections
        .iter()
        .map(|s| {
            (
                (!strip_anchors).then_some(s.section),
                s.level,
                s.heading_line.as_str(),
                s.body.as_str(),
            )
        })
        .collect();
    let ids: Vec<SectionId> = sections.iter().map(|s| s.section).collect();
    let markers_after = |i: usize| -> Vec<(SectionId, String)> {
        let Some(this) = ids.get(i) else {
            return vec![];
        };
        markers
            .iter()
            .filter(|(_, after, _)| match after {
                Some(a) => a == this,
                None => i == 0,
            })
            .map(|(s, _, r)| (*s, r.clone()))
            .collect()
    };
    // Markers whose `after` section no longer exists go after the preamble.
    let orphan: Vec<(SectionId, String)> = markers
        .iter()
        .filter(|(_, after, _)| after.map(|a| !ids.contains(&a)).unwrap_or(false))
        .map(|(s, _, r)| (*s, r.clone()))
        .collect();
    let body = wisp_parse::sections::render_body(&rendered, &le, &|i| {
        let mut v = markers_after(i);
        if i == 0 {
            v.extend(orphan.iter().cloned());
        }
        v
    });
    out.push_str(&body);
    if sections.is_empty() {
        // Nothing left to anchor the markers to: they follow the frontmatter.
        for (s, _, r) in &markers {
            out.push_str(&wisp_parse::sections::erasure_marker(*s, r));
            out.push_str(&le);
        }
    }
    Ok(out)
}

fn entry_string(entries: &[wisp_store::FrontmatterState], key: &str) -> Option<String> {
    entries
        .iter()
        .find(|e| e.key == key)
        .and_then(|e| wisp_parse::frontmatter::entry_string(&e.raw))
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// All directories that need an `index.md`/`log.md`: every directory containing concepts, plus ancestors and the root.
fn view_dirs(concepts: &[ConceptState]) -> BTreeSet<String> {
    let mut dirs = BTreeSet::new();
    dirs.insert(String::new());
    for c in concepts {
        let mut d = dir_of(&c.path);
        loop {
            dirs.insert(d.clone());
            if d.is_empty() {
                break;
            }
            d = dir_of(&d);
        }
    }
    dirs
}

pub struct RenderedViews {
    pub files: Vec<(String, String)>, // (relative file path, content)
}

/// Generate `index.md` and `log.md` for every directory (§8, OKF §3.1).
pub fn render_views(
    repo: &Repository,
    hot: &rusqlite::Connection,
    ref_name: &str,
) -> Result<RenderedViews> {
    let concepts = Hot::concepts(hot)?;
    let dirs = view_dirs(&concepts);
    let mut files = Vec::new();

    // Titles/descriptions.
    let mut info: BTreeMap<ConceptId, (Option<String>, Option<String>)> = BTreeMap::new();
    for c in &concepts {
        let fm = Hot::frontmatter_of(hot, c.concept)?;
        info.insert(
            c.concept,
            (entry_string(&fm, "title"), entry_string(&fm, "description")),
        );
    }

    // Log entries from the ref's history.
    let head = repo.head(ref_name)?;
    let chain = crate::apply::history_chain(&repo.ledger, head)?;
    let mut log: Vec<(Option<String>, LogEntry)> = Vec::new(); // (concept path or None for erasure)
    let path_of = |id: ConceptId| -> Option<String> {
        concepts
            .iter()
            .find(|c| c.concept == id)
            .map(|c| c.path.clone())
            .or_else(|| Hot::retracted(hot, id).ok().flatten().map(|r| r.0))
    };
    for row in &chain {
        let cs = &row.changeset;
        let date = cs.at.get(..10).unwrap_or("").to_string();
        let actor = repo.actor_display(cs.actor_id);
        let changes = Ledger::changes_of(repo.ledger.conn(), cs.id)?;
        let mut seen: BTreeSet<(Option<ConceptId>, &'static str)> = BTreeSet::new();
        let created: BTreeSet<ConceptId> = changes
            .iter()
            .filter(|c| c.op == Op::CreateConcept)
            .filter_map(|c| c.target.concept())
            .collect();
        for ch in &changes {
            if matches!(
                ch.op,
                Op::RegisterActor | Op::ActorKey | Op::GroupAdd | Op::GroupRemove
            ) {
                continue;
            }
            let word = ch.op.log_word();
            // The changeset that creates a concept logs one Creation, not Creation plus Update.
            if word == "Update"
                && ch
                    .target
                    .concept()
                    .map(|c| created.contains(&c))
                    .unwrap_or(false)
            {
                continue;
            }
            if ch.op == Op::Erase {
                if seen.insert((None, word)) {
                    log.push((
                        None,
                        LogEntry {
                            date: date.clone(),
                            at: cs.at.clone(),
                            word,
                            path: None,
                            title: None,
                            actor: actor.clone(),
                        },
                    ));
                }
                continue;
            }
            let Some(cid) = ch.target.concept() else {
                continue;
            };
            if !seen.insert((Some(cid), word)) {
                continue;
            }
            let Some(p) = path_of(cid) else { continue };
            let title = info.get(&cid).and_then(|(t, _)| t.clone());
            log.push((
                Some(p.clone()),
                LogEntry {
                    date: date.clone(),
                    at: cs.at.clone(),
                    word,
                    path: Some(p),
                    title,
                    actor: actor.clone(),
                },
            ));
        }
    }

    for dir in &dirs {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let here: Vec<DirEntry> = concepts
            .iter()
            .filter(|c| dir_of(&c.path) == *dir)
            .map(|c| {
                let (t, d) = info.get(&c.concept).cloned().unwrap_or((None, None));
                DirEntry {
                    path: c.path.clone(),
                    title: t,
                    description: d,
                    type_: c.type_.clone(),
                }
            })
            .collect();
        let subdirs: Vec<String> = dirs
            .iter()
            .filter(|d| !d.is_empty() && dir_of(d) == *dir)
            .cloned()
            .collect();
        let view = DirView {
            dir: dir.clone(),
            concepts: here,
            subdirs,
        };
        let idx = repo.profile.render_index(&view);
        let entries: Vec<LogEntry> = log
            .iter()
            .filter(|(p, _)| match p {
                None => dir.is_empty(),
                Some(p) => p.starts_with(&prefix) || dir.is_empty(),
            })
            .map(|(_, e)| e.clone())
            .collect();
        let lg = repo.profile.render_log(&view, &entries);
        files.push((format!("{prefix}index.md"), idx));
        files.push((format!("{prefix}log.md"), lg));
    }
    Ok(RenderedViews { files })
}

/// Render the whole view of a ref into `dir` (the working tree or a detached directory).
/// Returns the checkout index describing what was written.
pub fn write_tree(
    repo: &mut Repository,
    ref_name: &str,
    dir: &Path,
    strip_anchors: bool,
    include_retracted: bool,
    remove_stale: bool,
) -> Result<CheckoutIndex> {
    let head = repo.head(ref_name)?;
    repo.hot(ref_name)?;
    let hot = repo.hots.remove(ref_name).expect("hot");
    let res = write_tree_with(
        repo,
        &hot,
        ref_name,
        head,
        dir,
        strip_anchors,
        include_retracted,
        remove_stale,
    );
    repo.hots.insert(ref_name.to_string(), hot);
    res
}

#[allow(clippy::too_many_arguments)]
fn write_tree_with(
    repo: &Repository,
    hot: &Hot,
    ref_name: &str,
    head: Option<ChangesetId>,
    dir: &Path,
    strip_anchors: bool,
    include_retracted: bool,
    remove_stale: bool,
) -> Result<CheckoutIndex> {
    let conn = hot.conn();
    let mut idx = CheckoutIndex {
        ref_name: ref_name.to_string(),
        head,
        ..Default::default()
    };
    let concepts = Hot::concepts(conn)?;
    let mut written: BTreeSet<PathBuf> = BTreeSet::new();
    for c in &concepts {
        let text = render_concept(repo, conn, c, strip_anchors, None)?;
        let rel = format!("{}.md", c.path);
        let p = dir.join(&rel);
        write_atomic(&p, text.as_bytes())?;
        written.insert(p.clone());
        idx.files.insert(
            rel,
            file_record(&p, "concept", Some(c.concept.to_string()))?,
        );
        idx.concepts
            .insert(c.concept.to_string(), concept_base(conn, c)?);
    }
    if include_retracted {
        for (cid, path) in Hot::retracted_all(conn)? {
            // Render from the cold tier by replaying into a scratch hot.
            let scratch = tempfile_hot()?;
            let reader = repo.reader();
            crate::apply::replay_concept(&reader, scratch.conn(), cid)?;
            if let Some(c) = Hot::concept(scratch.conn(), cid)? {
                let (_, at, actor, reason) = Hot::retracted(conn, cid)?.unwrap_or((
                    path.clone(),
                    String::new(),
                    ActorId::new(),
                    "other".into(),
                ));
                let mut c2 = c.clone();
                c2.status = Some(Status::Deprecated);
                let text = render_concept(
                    repo,
                    scratch.conn(),
                    &c2,
                    strip_anchors,
                    Some((at, repo.actor_display(actor), reason)),
                )?;
                let rel = format!("{}.md", path);
                let p = dir.join(&rel);
                write_atomic(&p, text.as_bytes())?;
                written.insert(p);
            }
        }
    }
    for a in Hot::assets(conn)? {
        let p = dir.join(&a.path);
        write_atomic(&p, &a.bytes)?;
        written.insert(p.clone());
        idx.files.insert(
            a.path.clone(),
            file_record(&p, "asset", Some(a.asset.to_string()))?,
        );
        idx.assets.insert(a.path.clone(), (a.asset, a.version));
    }
    let views = render_views(repo, conn, ref_name)?;
    for (rel, content) in views.files {
        let p = dir.join(&rel);
        write_atomic(&p, content.as_bytes())?;
        written.insert(p.clone());
        idx.files.insert(rel, file_record(&p, "generated", None)?);
    }
    if remove_stale {
        // Remove tracked files that no longer exist in the view (retracted concepts, old generated files).
        if let Some(old) = CheckoutIndex::load(&checkout_idx_path(repo, dir))? {
            for (rel, _) in old.files {
                let p = dir.join(&rel);
                if !written.contains(&p) && p.exists() {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
    Ok(idx)
}

fn tempfile_hot() -> Result<Hot> {
    let dir = std::env::temp_dir().join(format!("wisp-scratch-{}", ChangeId::new()));
    std::fs::create_dir_all(&dir)?;
    Hot::open(&dir.join("hot.db"))
}

pub fn concept_base(conn: &rusqlite::Connection, c: &ConceptState) -> Result<ConceptBase> {
    Ok(ConceptBase {
        path: c.path.clone(),
        version: c.version,
        content_version: c.content_version,
        sections: Hot::sections_of(conn, c.concept)?
            .into_iter()
            .map(|s| (s.section, s.version))
            .collect(),
        entries: Hot::frontmatter_of(conn, c.concept)?
            .into_iter()
            .map(|e| (e.key, e.version))
            .collect(),
        sources: Hot::sources_of(conn, c.concept)?
            .into_iter()
            .map(|s| (s.source_id, s.version))
            .collect(),
    })
}

/// An advisory lock serializing working-tree rewrites across processes (a SQLite `BEGIN IMMEDIATE`
/// on a dedicated file). Released on drop.
pub struct WorktreeLock {
    conn: rusqlite::Connection,
}

impl WorktreeLock {
    pub fn acquire(wisp_dir: &Path) -> Result<Self> {
        let conn = rusqlite::Connection::open(wisp_dir.join("worktree.lock"))
            .map_err(|e| WispError::internal(e.to_string()))?;
        conn.busy_timeout(std::time::Duration::from_millis(10_000))
            .map_err(|e| WispError::internal(e.to_string()))?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS l (x INTEGER); BEGIN IMMEDIATE;")
            .map_err(|_| WispError::timeout("working tree is locked by another process"))?;
        Ok(WorktreeLock { conn })
    }
}

impl Drop for WorktreeLock {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("COMMIT;");
    }
}

/// `checkout.idx` lives in `.wisp/` for the main working tree.
pub fn checkout_idx_path(repo: &Repository, dir: &Path) -> PathBuf {
    if dir == repo.root {
        repo.wisp_dir.join("checkout.idx")
    } else {
        dir.join(".wisp-checkout.idx")
    }
}

/// `wisp checkout`: render the ref into the working tree (or `--to dir`) and record `checkout.idx`.
pub fn checkout(
    repo: &mut Repository,
    ref_name: &str,
    to: Option<&Path>,
    strip_anchors: bool,
    include_retracted: bool,
) -> Result<CheckoutIndex> {
    let dir = to
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.root.clone());
    std::fs::create_dir_all(&dir)?;
    let idx = write_tree(
        repo,
        ref_name,
        &dir,
        strip_anchors,
        include_retracted,
        to.is_none(),
    )?;
    if to.is_none() {
        idx.save(&checkout_idx_path(repo, &dir))?;
    }
    Ok(idx)
}

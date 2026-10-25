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

fn remove_managed_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WispError::internal(format!(
            "remove {}: {e}",
            path.display()
        ))),
    }
}

/// Drop working-tree files that were tracked before a rewrite but are absent from the new index
/// (for example when a purged concept no longer appears in hot).
pub fn prune_tracked_files_removed_from_index(
    dir: &Path,
    prior: Option<&CheckoutIndex>,
    new_idx: &CheckoutIndex,
) -> Result<()> {
    if let Some(old) = prior {
        for rel in old.files.keys() {
            if !new_idx.files.contains_key(rel) {
                let p = dir.join(rel);
                remove_managed_file(&p)?;
            }
        }
    }
    Ok(())
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

fn profile_for_render<'a>(
    repo: &'a Repository,
    export_profile: Option<&'a dyn wisp_profile::Profile>,
) -> &'a dyn wisp_profile::Profile {
    export_profile.unwrap_or(&*repo.profile)
}

/// Resolve `--profile` for down-export (0.2 → 0.1). Same profile as the repo returns `None`.
pub fn export_profile_for(
    repo: &Repository,
    profile_arg: &str,
) -> Result<Option<Box<dyn wisp_profile::Profile>>> {
    let target = wisp_profile::by_version(profile_arg)
        .or_else(|| wisp_profile::by_id(profile_arg))
        .ok_or_else(|| WispError::usage(format!("unknown profile {profile_arg}")))?;
    if target.id() == repo.profile.id() {
        return Ok(None);
    }
    if repo.profile.okf_version() == "0.2" && target.okf_version() == "0.1" {
        return Ok(Some(target));
    }
    Err(WispError::usage(format!(
        "cannot render {} bundle as {} (down-export supports 0.2 → 0.1 only)",
        repo.profile.id(),
        target.id(),
    )))
}

fn bundle_resource(path: &str) -> String {
    format!("/{}.md", path.trim_start_matches('/'))
}

fn merge_source_entry(base: &serde_json::Value, fill: &serde_json::Value) -> serde_json::Value {
    let mut out = base.clone();
    if let Some(obj) = fill.as_object() {
        if let Some(base_map) = out.as_object_mut() {
            for (k, v) in obj {
                let empty = base_map
                    .get(k)
                    .is_none_or(|x| x.is_null() || (x.is_string() && x.as_str() == Some("")));
                if empty {
                    base_map.insert(k.clone(), v.clone());
                }
            }
        }
    }
    out
}

fn resource_key(rec: &serde_json::Value) -> Option<String> {
    rec.get("resource")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn projected_sources_from_inputs(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
) -> Result<Vec<serde_json::Value>> {
    let cv = match c.content_version {
        Some(v) => v,
        None => return Ok(vec![]),
    };
    let change = Ledger::get_change(repo.ledger.conn(), cv)?;
    let Some(change) = change else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for inp in &change.inputs {
        if let InputRef::Concept {
            concept: ic,
            section,
            ..
        } = inp
        {
            let Some(src_concept) = Hot::concept(hot, *ic)? else {
                continue;
            };
            let resource = bundle_resource(&src_concept.path);
            let mut entry = serde_json::json!({ "resource": resource });
            if let Some(sec) = section {
                let Some(sec_row) = Hot::section(hot, *sec)? else {
                    continue;
                };
                if sec_row.concept != *ic {
                    continue;
                }
                entry["id"] = serde_json::Value::String(sec.to_string());
                entry["section"] = serde_json::Value::String(sec.to_string());
                if !sec_row.heading.is_empty() {
                    entry["title"] = serde_json::Value::String(sec_row.heading.clone());
                }
            }
            out.push(entry);
        }
    }
    Ok(out)
}

fn merge_sources_for_checkout(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
    stored: Vec<wisp_store::SourceState>,
) -> Result<Vec<serde_json::Value>> {
    let mut by_resource: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for s in stored {
        let rec = s.record;
        if let Some(key) = resource_key(&rec) {
            if !by_resource.contains_key(&key) {
                order.push(key.clone());
            }
            by_resource.insert(key, rec);
        } else {
            let key = format!("id:{}", s.source_id);
            order.push(key.clone());
            by_resource.insert(key, rec);
        }
    }
    for proj in projected_sources_from_inputs(repo, hot, c)? {
        if let Some(key) = resource_key(&proj) {
            if let Some(existing) = by_resource.get(&key) {
                by_resource.insert(key, merge_source_entry(existing, &proj));
            } else {
                order.push(key.clone());
                by_resource.insert(key, proj);
            }
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|k| by_resource.get(&k).cloned())
        .collect())
}

/// Build the profile's view of a concept from the hot tier.
pub fn concept_view(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
    include_retracted: Option<(String, String, String)>,
    export_profile: Option<&dyn wisp_profile::Profile>,
) -> Result<ConceptView> {
    let managed: &[&str] = profile_for_render(repo, export_profile).managed_keys();
    let entries: Vec<(String, String)> = Hot::frontmatter_of(hot, c.concept)?
        .into_iter()
        .filter(|e| !managed.contains(&e.key.as_str()) && e.key != repo.identity_key())
        .map(|e| (e.key, e.raw))
        .collect();
    let sections: Vec<(u8, String, String)> = Hot::sections_of(hot, c.concept)?
        .into_iter()
        .map(|s| (s.level, s.heading, s.body))
        .collect();
    let sources = merge_sources_for_checkout(repo, hot, c, Hot::sources_of(hot, c.concept)?)?;
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
        source_ids: sources
            .iter()
            .filter_map(|s| s.get("id").and_then(|x| x.as_str()).map(String::from))
            .collect(),
        sources,
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

fn append_citations_downexport(
    out: &mut String,
    view: &ConceptView,
    sections: &[wisp_store::SectionState],
    le: &str,
    export_profile: Option<&dyn wisp_profile::Profile>,
    repo_profile: &dyn wisp_profile::Profile,
) {
    let prof = export_profile.unwrap_or(repo_profile);
    if prof.okf_version() != "0.1" || repo_profile.okf_version() != "0.2" || view.sources.is_empty()
    {
        return;
    }
    if sections
        .iter()
        .any(|s| s.heading.eq_ignore_ascii_case("citations"))
    {
        return;
    }
    out.push_str(&format!("## Citations{le}{le}"));
    for src in &view.sources {
        if let Some(t) = src
            .get("title")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("- {t}{le}"));
        }
    }
}

/// Render a concept file: frontmatter via the profile, then the sections with anchors and erasure markers.
pub fn render_concept(
    repo: &Repository,
    hot: &rusqlite::Connection,
    c: &ConceptState,
    strip_anchors: bool,
    retracted: Option<(String, String, String)>,
    export_profile: Option<&dyn wisp_profile::Profile>,
) -> Result<String> {
    let prof = profile_for_render(repo, export_profile);
    let view = concept_view(repo, hot, c, retracted, export_profile)?;
    let le = view.line_ending.clone();
    let mut out = prof.render_frontmatter(&view, repo.identity_key());
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
    append_citations_downexport(
        &mut out,
        &view,
        &sections,
        &le,
        export_profile,
        &*repo.profile,
    );
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
    export_profile: Option<&dyn wisp_profile::Profile>,
) -> Result<RenderedViews> {
    let prof = profile_for_render(repo, export_profile);
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
        let idx = prof.render_index(&view);
        let entries: Vec<LogEntry> = log
            .iter()
            .filter(|(p, _)| match p {
                None => dir.is_empty(),
                Some(p) => p.starts_with(&prefix) || dir.is_empty(),
            })
            .map(|(_, e)| e.clone())
            .collect();
        let lg = prof.render_log(&view, &entries);
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
    write_tree_export_profile(
        repo,
        ref_name,
        dir,
        strip_anchors,
        include_retracted,
        remove_stale,
        None,
    )
}

pub fn write_tree_export_profile(
    repo: &mut Repository,
    ref_name: &str,
    dir: &Path,
    strip_anchors: bool,
    include_retracted: bool,
    remove_stale: bool,
    export_profile: Option<&dyn wisp_profile::Profile>,
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
        export_profile,
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
    export_profile: Option<&dyn wisp_profile::Profile>,
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
        let text = render_concept(repo, conn, c, strip_anchors, None, export_profile)?;
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
                    export_profile,
                )?;
                let rel = format!("{path}.md");
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
    let views = render_views(repo, conn, ref_name, export_profile)?;
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

/// Materialize a publish set into a managed tree (deferred drain / delta publish).
pub fn publish_ref_delta(
    repo: &mut Repository,
    ref_name: &str,
    tree_root: &Path,
    dirty: &crate::publish::DirtySet,
) -> Result<Vec<String>> {
    if dirty.is_empty() {
        return Ok(vec![]);
    }
    let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;
    let idx_path = checkout_idx_path(repo, tree_root);
    let head = repo.head(ref_name)?;
    let mut idx = match CheckoutIndex::load(&idx_path)? {
        Some(i) if i.ref_name == ref_name => i,
        Some(_) => return Ok(vec![]),
        None => CheckoutIndex {
            ref_name: ref_name.to_string(),
            head,
            ..Default::default()
        },
    };
    let mut disk_dirty: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (rel, rec) in &idx.files {
        if file_changed(&tree_root.join(rel), rec) {
            disk_dirty.insert(rel.clone());
        }
    }
    repo.hot(ref_name)?;
    let hot = repo.hots.get(ref_name).expect("hot");
    let conn = hot.conn();
    let mut rewritten = Vec::new();

    for path in &dirty.concept_paths {
        let rel = format!("{path}.md");
        if !dirty.reconcile && disk_dirty.contains(&rel) {
            continue;
        }
        let Some(c) = Hot::concept_by_path(conn, path)? else {
            continue;
        };
        let text = render_concept(repo, conn, &c, false, None, None)?;
        let dst = tree_root.join(&rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = text.as_bytes();
        let same = std::fs::read(&dst).map(|b| b == bytes).unwrap_or(false);
        if !same {
            write_atomic(&dst, bytes)?;
            rewritten.push(rel.clone());
        }
        idx.files.insert(
            rel,
            file_record(&dst, "concept", Some(c.concept.to_string()))?,
        );
        idx.concepts
            .insert(c.concept.to_string(), concept_base(conn, &c)?);
    }

    for ap in &dirty.asset_paths {
        if !dirty.reconcile && disk_dirty.contains(ap) {
            continue;
        }
        let Some(a) = Hot::assets(conn)?.into_iter().find(|x| x.path == *ap) else {
            continue;
        };
        let dst = tree_root.join(ap);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(&dst, &a.bytes)?;
        rewritten.push(ap.clone());
        idx.files.insert(
            ap.clone(),
            file_record(&dst, "asset", Some(a.asset.to_string()))?,
        );
        idx.assets.insert(ap.clone(), (a.asset, a.version));
    }

    if !dirty.view_dirs.is_empty() {
        let views = render_views(repo, conn, ref_name, None)?;
        for (rel, content) in views.files {
            let dir = view_dir_of_generated(&rel);
            if !dirty.view_dirs.contains(&dir) {
                continue;
            }
            if !dirty.reconcile && disk_dirty.contains(&rel) {
                continue;
            }
            let dst = tree_root.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_atomic(&dst, content.as_bytes())?;
            rewritten.push(rel.clone());
            idx.files.insert(rel, file_record(&dst, "generated", None)?);
        }
    }

    for rel in &dirty.prune_rels {
        if !dirty.reconcile && disk_dirty.contains(rel) {
            continue;
        }
        let p = tree_root.join(rel);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
            rewritten.push(rel.clone());
        }
        idx.files.remove(rel);
        idx.concepts.retain(|_, b| format!("{}.md", b.path) != *rel);
    }

    idx.head = head;
    idx.save(&idx_path)?;
    Ok(rewritten)
}

fn view_dir_of_generated(rel: &str) -> String {
    if rel == "index.md" || rel == "log.md" {
        return String::new();
    }
    match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    }
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
            .map_err(|_| {
                WispError::write_timeout(
                    "working tree is locked by another process",
                    wisp_model::WriteTimeoutDetails {
                        lock: "worktree_flock".into(),
                        phase: "worktree.flock_begin_immediate".into(),
                        busy_timeout_ms: Some(10_000),
                        hint: Some(
                            "Another process holds .wisp/worktree.lock; retry after it finishes materialize or record."
                                .into(),
                        ),
                        ..Default::default()
                    },
                )
            })?;
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
    checkout_with_profile(repo, ref_name, to, None, strip_anchors, include_retracted)
}

/// Checkout or export to a directory, optionally down-exporting to an older OKF profile.
pub fn checkout_with_profile(
    repo: &mut Repository,
    ref_name: &str,
    to: Option<&Path>,
    profile: Option<&str>,
    strip_anchors: bool,
    include_retracted: bool,
) -> Result<CheckoutIndex> {
    let export_profile = match profile {
        None => None,
        Some(p) => export_profile_for(repo, p)?,
    };
    if export_profile.is_some() && to.is_none() {
        return Err(WispError::usage(
            "checkout --profile for down-export requires --to <dir>",
        ));
    }
    let dir = to
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.root.clone());
    std::fs::create_dir_all(&dir)?;
    let idx = if let Some(ref prof) = export_profile {
        write_tree_export_profile(
            repo,
            ref_name,
            &dir,
            strip_anchors,
            include_retracted,
            false,
            Some(prof.as_ref()),
        )?
    } else {
        write_tree(
            repo,
            ref_name,
            &dir,
            strip_anchors,
            include_retracted,
            to.is_none(),
        )?
    };
    if to.is_none() {
        idx.save(&checkout_idx_path(repo, &dir))?;
    }
    Ok(idx)
}

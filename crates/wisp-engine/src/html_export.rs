//! `wisp export --html`: collect a ref-head view and render static HTML.

use crate::checkout::WorktreeLock;
use crate::payloads::SectionBody;
use crate::repo::Repository;
use crate::snapshot::{is_section_version, History, Unit};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use wisp_html::{
    line_diff, markdown_to_html, redact_site_model, render_provenance_svg, render_receipt,
    render_site, verify_site_dir, ActivityRow, ActorIndexRow, ConceptLink, ConceptPage,
    ErasureMarkerRow, HtmlExportOptions, InputRow, MergeIndexRow, Overview, ProvenanceGraph,
    ReceiptSignature, RemovalIndexRow, RetractionRow, SectionLink, SectionPage, SectionVersion,
    SiteModel, HISTORY_PAGE_SIZE, MAX_INPUTS_PER_VERSION,
};
use wisp_model::*;
use wisp_store::{Hot, Ledger};

pub fn verify_html_export(dest: &Path) -> Result<Value> {
    verify_site_dir(dest)
}

pub fn export_html(
    repo: &mut Repository,
    ref_name: &str,
    dest: &Path,
    opts: HtmlExportOptions,
) -> Result<Value> {
    validate_destination(repo, dest)?;
    if dest.exists() {
        return Err(WispError::usage(format!(
            "export destination already exists: {}",
            dest.display()
        )));
    }
    if !Ledger::pending_erasures(repo.ledger.conn())?.is_empty() {
        return Err(WispError::usage(
            "cannot export while erasure recovery is pending; reopen the repository or complete recovery first",
        ));
    }

    let _lock = WorktreeLock::acquire(&repo.wisp_dir)?;

    repo.hot(ref_name)?;
    let head = repo
        .head(ref_name)?
        .ok_or_else(|| WispError::usage(format!("ref {ref_name} has no head")))?;
    let since_cs = parse_since(opts.since.as_deref())?;
    let mut model = collect(&*repo, ref_name, &opts, since_cs)?;
    redact_site_model(&mut model);
    let parent = dest
        .parent()
        .ok_or_else(|| WispError::usage("invalid export path"))?;
    let staging = tempfile::tempdir_in(parent).map_err(|e| WispError::internal(e.to_string()))?;
    let summary = render_site(&model, staging.path())?;

    let export_id = PayloadId::new();
    let at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| WispError::internal(e.to_string()))?;

    let staging_path = staging.path().to_path_buf();

    let signature = if opts.sign {
        sign_manifest(repo, &summary.manifest_sha256)?
    } else {
        None
    };
    render_receipt(
        &staging_path,
        &export_id.to_string(),
        &at,
        &summary.manifest_sha256,
        signature.as_ref(),
    )?;

    fs::rename(&staging_path, dest)
        .map_err(|e| WispError::internal(format!("publish export to {}: {}", dest.display(), e)))?;

    let tx = repo.ledger.begin_immediate()?;
    recheck_export_view_conn(&tx, ref_name, head)?;
    Ledger::insert_export(
        &tx,
        &wisp_store::ExportRow {
            id: export_id,
            changeset: head,
            kind: "html".to_string(),
            at: at.clone(),
            destination: Some(dest.display().to_string()),
            manifest_sha256: Some(summary.manifest_sha256.clone()),
            status: "published".to_string(),
        },
    )?;
    tx.commit()
        .map_err(|e| WispError::internal(e.to_string()))?;

    Ok(json!({
        "kind": "html",
        "dir": dest.display().to_string(),
        "ref": ref_name,
        "head": head.to_string(),
        "content_mode": if opts.content { "full" } else { "no_content" },
        "ids_only": opts.ids_only,
        "since": opts.since,
        "signed": signature.is_some(),
        "export_id": export_id.to_string(),
        "page_count": summary.page_count,
        "manifest_sha256": summary.manifest_sha256,
        "warnings": ["plaintext_copy"],
    }))
}

fn validate_destination(repo: &Repository, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| WispError::usage("invalid export path"))?;
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let root = fs::canonicalize(&repo.root)?;
    let wisp = fs::canonicalize(&repo.wisp_dir)?;
    if dest.is_symlink() || parent.is_symlink() {
        return Err(WispError::usage("export path must not use symlinks"));
    }
    let resolved = parent.join(
        dest.file_name()
            .ok_or_else(|| WispError::usage("invalid export path"))?,
    );
    if resolved.starts_with(&root) {
        return Err(WispError::usage(
            "export destination must be outside the wisp bundle root",
        ));
    }
    if resolved.starts_with(&wisp) {
        return Err(WispError::usage(
            "export destination must not be under .wisp/",
        ));
    }
    Ok(())
}

fn recheck_export_view_conn(
    conn: &rusqlite::Connection,
    ref_name: &str,
    head: ChangesetId,
) -> Result<()> {
    let now = Ledger::head(conn, ref_name)?
        .ok_or_else(|| WispError::usage(format!("ref {ref_name} has no head")))?;
    if now != head {
        return Err(WispError::usage(
            "repository changed during export; retry the export",
        ));
    }
    if !Ledger::pending_erasures(conn)?.is_empty() {
        return Err(WispError::usage(
            "erasure recovery became pending during export; retry",
        ));
    }
    Ok(())
}

struct HotSnap {
    concept_paths: HashMap<ConceptId, String>,
    concept_scopes: HashMap<ConceptId, ScopeId>,
    retracted: HashMap<ConceptId, String>,
    section_headings: HashMap<SectionId, String>,
}

fn parse_since(since: Option<&str>) -> Result<Option<ChangesetId>> {
    match since {
        None => Ok(None),
        Some(s) => {
            Ok(Some(s.parse().map_err(|_| {
                WispError::usage(format!("bad changeset id {s:?}"))
            })?))
        }
    }
}

fn sign_manifest(repo: &Repository, manifest_sha256: &str) -> Result<Option<ReceiptSignature>> {
    let actor_name = repo.default_actor();
    let row = repo
        .find_actor(&actor_name)?
        .ok_or_else(|| WispError::usage("export --sign requires a registered default actor"))?;
    let digest = hex::decode(manifest_sha256).map_err(|_| WispError::internal("manifest hash"))?;
    let digest: [u8; 32] = digest
        .try_into()
        .map_err(|_| WispError::internal("manifest hash length"))?;
    let reader = repo.reader();
    let sig = crate::signing::sign_digest(&repo.wisp_dir, &reader, row.id, &digest)?;
    Ok(sig.map(|bytes| ReceiptSignature {
        actor: actor_name,
        bytes_hex: hex::encode(bytes),
    }))
}

fn in_export_window(
    history: &mut History,
    head: ChangesetId,
    since: Option<ChangesetId>,
    cs: ChangesetId,
) -> Result<bool> {
    if !history.is_ancestor_or_self(cs, head)? {
        return Ok(false);
    }
    if let Some(s) = since {
        if cs != s && !history.is_ancestor_or_self(s, cs)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn collect(
    repo: &Repository,
    ref_name: &str,
    opts: &HtmlExportOptions,
    since: Option<ChangesetId>,
) -> Result<SiteModel> {
    let content = opts.content;
    let head = repo.head(ref_name)?;
    let Some(head_cs) = head else {
        return Err(WispError::usage(format!("ref {ref_name} has no head")));
    };
    let mut history = History::new(repo.ledger.conn())?;

    let (concepts_state, retracted, concept_markdown, sections_by_concept, hot_snap, erasures) = {
        let hot = repo
            .hots
            .get(ref_name)
            .ok_or_else(|| WispError::internal("hot tier not open"))?;
        let conn = hot.conn();
        let concepts_state = Hot::concepts(conn)?;
        let retracted = Hot::retracted_all(conn)?;
        let mut concept_paths = HashMap::new();
        let mut concept_scopes = HashMap::new();
        for c in &concepts_state {
            concept_paths.insert(c.concept, c.path.clone());
            concept_scopes.insert(c.concept, c.scope);
        }
        let retracted_map: HashMap<ConceptId, String> = retracted.iter().cloned().collect();
        let mut erasures = Vec::new();
        let mut hot_snap = HotSnap {
            concept_paths,
            concept_scopes,
            retracted: retracted_map,
            section_headings: HashMap::new(),
        };
        let mut concept_markdown: Vec<(ConceptId, String)> = Vec::new();
        let mut sections_by_concept: Vec<(ConceptId, Vec<wisp_store::SectionState>)> = Vec::new();
        for c in &concepts_state {
            let sections = Hot::sections_of(conn, c.concept)?;
            for s in &sections {
                hot_snap
                    .section_headings
                    .insert(s.section, s.heading.clone());
            }
            for (section, _after, reason) in Hot::erased_markers(conn, c.concept)? {
                erasures.push(ErasureMarkerRow {
                    concept_id: c.concept.to_string(),
                    concept_path: c.path.clone(),
                    section_id: section.to_string(),
                    reason: reason.as_str().to_string(),
                });
            }
            sections_by_concept.push((c.concept, sections));
            if content {
                concept_markdown.push((c.concept, render_concept_markdown(conn, c.concept)?));
            }
        }
        erasures.sort_by(|a, b| {
            a.concept_path
                .cmp(&b.concept_path)
                .then_with(|| a.section_id.cmp(&b.section_id))
        });
        (
            concepts_state,
            retracted,
            concept_markdown,
            sections_by_concept,
            hot_snap,
            erasures,
        )
    };

    let mut demo_concept: Option<ConceptId> = None;
    for c in &concepts_state {
        if c.path == "notes/webhook-setup.md" {
            demo_concept = Some(c.concept);
        }
    }

    let mut section_count = 0usize;
    let mut concept_pages = Vec::new();
    let mut section_summaries = Vec::new();
    let mut section_pages = Vec::new();
    let mut graphs = Vec::new();
    let mut overview_links = Vec::new();

    for c in &concepts_state {
        overview_links.push(ConceptLink {
            id: c.concept.to_string(),
            path: c.path.clone(),
            status: c.status.map(|s| s.as_str().to_string()),
        });

        let sections = sections_by_concept
            .iter()
            .find(|(id, _)| *id == c.concept)
            .map(|(_, s)| s)
            .cloned()
            .unwrap_or_default();
        section_count += sections.len();
        let mut section_links = Vec::new();
        for s in &sections {
            section_links.push(SectionLink {
                id: s.section.to_string(),
                heading: s.heading.clone(),
            });
            let pages = collect_section_pages(
                repo,
                &hot_snap,
                &mut history,
                head_cs,
                since,
                c,
                s,
                content,
            )?;
            if let Some(first) = pages.first() {
                section_summaries.push(first.clone());
                if content && !first.versions.is_empty() {
                    let inputs = first.versions[0].inputs.clone();
                    let (svg, truncated) = render_provenance_svg(
                        &first.id,
                        &first.versions[0].change_id,
                        &inputs,
                        opts.graph_max_nodes,
                    );
                    graphs.push(ProvenanceGraph {
                        section_id: first.id.clone(),
                        version_id: first.versions[0].change_id.clone(),
                        svg,
                        truncated,
                    });
                }
            }
            section_pages.extend(pages);
        }

        let markdown_html = if content {
            concept_markdown
                .iter()
                .find(|(id, _)| *id == c.concept)
                .map(|(_, md)| markdown_to_html(md))
        } else {
            None
        };

        let mut badges = Vec::new();
        if let Some(st) = c.status {
            badges.push(format!("status: {}", st.as_str()));
        }
        if erasures
            .iter()
            .any(|e| e.concept_id == c.concept.to_string())
        {
            badges.push("erasure marker".to_string());
        }
        concept_pages.push(ConceptPage {
            id: c.concept.to_string(),
            path: c.path.clone(),
            type_name: c.type_.clone().unwrap_or_else(|| "note".to_string()),
            status: c.status.map(|s| s.as_str().to_string()),
            retracted: false,
            markdown_html,
            sections: section_links,
            badges,
        });
    }

    let retractions: Vec<RetractionRow> = {
        let hot = repo
            .hots
            .get(ref_name)
            .ok_or_else(|| WispError::internal("hot tier not open"))?;
        let conn = hot.conn();
        let mut rows: Vec<RetractionRow> = retracted
            .into_iter()
            .map(|(id, path)| {
                let reason = Hot::retracted(conn, id)
                    .ok()
                    .flatten()
                    .map(|(_, _, _, r)| r);
                RetractionRow {
                    concept_id: id.to_string(),
                    path,
                    reason,
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.concept_id.cmp(&b.concept_id))
        });
        rows
    };

    for row in &retractions {
        if concept_pages.iter().any(|p| p.id == row.concept_id) {
            continue;
        }
        concept_pages.push(ConceptPage {
            id: row.concept_id.clone(),
            path: row.path.clone(),
            type_name: "retracted".to_string(),
            status: None,
            retracted: true,
            markdown_html: None,
            sections: vec![],
            badges: vec!["retracted".to_string()],
        });
    }

    concept_pages.sort_by(|a, b| a.path.cmp(&b.path));
    section_pages.sort_by(|a, b| {
        a.concept_path
            .cmp(&b.concept_path)
            .then_with(|| a.heading.cmp(&b.heading))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.page.cmp(&b.page))
    });
    section_summaries.sort_by(|a, b| {
        a.concept_path
            .cmp(&b.concept_path)
            .then_with(|| a.heading.cmp(&b.heading))
            .then_with(|| a.id.cmp(&b.id))
    });
    overview_links.sort_by(|a, b| a.path.cmp(&b.path));

    let activity = collect_activity(repo, &hot_snap, &mut history, head_cs, since)?;
    let actors = collect_actors(repo, &mut history, head_cs, since)?;
    let merges = collect_merges(repo, &mut history, head_cs, since)?;
    let removals = collect_removals(&retractions, &erasures);

    Ok(SiteModel {
        ref_name: ref_name.to_string(),
        head: Some(head_cs.to_string()),
        content,
        ids_only: opts.ids_only,
        since: opts.since.clone(),
        overview: Overview {
            concept_count: concepts_state.len(),
            section_count,
            concepts: overview_links,
            retractions,
            erasures,
            demo_concept_id: demo_concept.map(|c| c.to_string()),
        },
        concepts: concept_pages,
        sections: section_summaries,
        section_pages,
        activity,
        actors,
        merges,
        removals,
        graphs,
    })
}

fn render_concept_markdown(conn: &rusqlite::Connection, concept: ConceptId) -> Result<String> {
    let st = Hot::concept(conn, concept)?.ok_or_else(|| WispError::not_found("concept"))?;
    let sections = Hot::sections_of(conn, concept)?;
    let mut out = String::new();
    let fm = Hot::frontmatter_of(conn, concept)?;
    if !fm.is_empty() {
        out.push_str("---\n");
        for e in &fm {
            out.push_str(&e.key);
            out.push_str(": ");
            out.push_str(&e.raw);
            out.push('\n');
        }
        out.push_str("---\n\n");
    }
    for s in sections {
        out.push_str(&"#".repeat(s.level as usize));
        out.push(' ');
        out.push_str(&s.heading);
        out.push_str("\n\n");
        out.push_str(&s.body);
        out.push_str("\n\n");
    }
    if out.is_empty() {
        out = format!("# {}\n", st.path);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn collect_section_pages(
    repo: &Repository,
    hot: &HotSnap,
    history: &mut History,
    head: ChangesetId,
    since: Option<ChangesetId>,
    concept: &wisp_store::ConceptState,
    section: &wisp_store::SectionState,
    content: bool,
) -> Result<Vec<SectionPage>> {
    let unit = Unit::Section(section.section);
    let rows = history.unit_rows(unit)?;
    let mut visible: Vec<&Change> = Vec::new();
    for (_, ch) in rows.iter() {
        if !is_section_version(ch.op) {
            continue;
        }
        if !in_export_window(history, head, since, ch.changeset)? {
            continue;
        }
        visible.push(ch);
    }
    visible.sort_by(|a, b| {
        let sa = history.seq(a.changeset).unwrap_or(None).unwrap_or(0);
        let sb = history.seq(b.changeset).unwrap_or(None).unwrap_or(0);
        sb.cmp(&sa).then_with(|| b.ordinal.cmp(&a.ordinal))
    });
    let total = visible.len();
    let page_count = total.div_ceil(HISTORY_PAGE_SIZE).max(1) as u32;
    let mut pages = Vec::new();
    for page_idx in 0..page_count {
        let start = page_idx as usize * HISTORY_PAGE_SIZE;
        let end = (start + HISTORY_PAGE_SIZE).min(total);
        let slice = visible[start..end].iter();
        let mut versions = Vec::new();
        for ch in slice {
            let skipped = history.skipped_at(ch.id, head)?;
            let not_kept = history.merge_skips_at(ch.id, head)?;

            let (actor, at) = changeset_meta(repo, ch.changeset)?;
            let inputs = format_inputs(repo, hot, history, head, ch)?;
            let (body, diff, word_diff, diff_missing) = if content {
                section_version_body_and_diff(repo, history, ch, skipped)?
            } else {
                (None, None, None, false)
            };

            versions.push(SectionVersion {
                change_id: ch.id.to_string(),
                changeset_id: ch.changeset.to_string(),
                actor,
                at,
                inputs_complete: ch.inputs_complete,
                inputs,
                inputs_truncated: ch.inputs.len() > MAX_INPUTS_PER_VERSION,
                not_kept_at_merge: not_kept,
                body,
                diff_vs_predecessor: diff,
                word_diff_vs_predecessor: word_diff,
                diff_missing_baseline: diff_missing,
            });
        }

        let page = page_idx + 1;
        pages.push(SectionPage {
            id: section.section.to_string(),
            concept_id: concept.concept.to_string(),
            concept_path: concept.path.clone(),
            heading: section.heading.clone(),
            versions,
            total_versions: total,
            versions_truncated: page_count > 1,
            page,
            page_count,
        });
    }
    Ok(pages)
}

fn change_target_label(ch: &Change, hot: &HotSnap) -> String {
    match &ch.target {
        Target::Section { section, .. } => hot
            .section_headings
            .get(section)
            .cloned()
            .unwrap_or_else(|| section.to_string()),
        Target::Concept { concept } => hot
            .concept_paths
            .get(concept)
            .cloned()
            .unwrap_or_else(|| concept.to_string()),
        _ => format!("{} {:?}", ch.op.as_str(), ch.target),
    }
}

fn collect_activity(
    repo: &Repository,
    hot_snap: &HotSnap,
    history: &mut History,
    head: ChangesetId,
    since: Option<ChangesetId>,
) -> Result<Vec<ActivityRow>> {
    let mut out = Vec::new();
    for (_, ch) in Ledger::changes_all(repo.ledger.conn())? {
        if !in_export_window(history, head, since, ch.changeset)? {
            continue;
        }
        let (actor, at) = changeset_meta(repo, ch.changeset)?;
        let (concept_id, section_id) = match &ch.target {
            Target::Section {
                concept, section, ..
            } => (Some(concept.to_string()), Some(section.to_string())),
            Target::Concept { concept } => (Some(concept.to_string()), None),
            _ => (None, None),
        };
        out.push(ActivityRow {
            changeset_id: ch.changeset.to_string(),
            change_id: ch.id.to_string(),
            at,
            actor,
            op: ch.op.as_str().to_string(),
            target: change_target_label(&ch, hot_snap),
            concept_id,
            section_id,
        });
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.change_id.cmp(&b.change_id)));
    Ok(out)
}

fn collect_actors(
    repo: &Repository,
    history: &mut History,
    head: ChangesetId,
    since: Option<ChangesetId>,
) -> Result<Vec<ActorIndexRow>> {
    let mut counts: HashMap<ActorId, usize> = HashMap::new();
    let mut seen_cs: HashSet<ChangesetId> = HashSet::new();
    for (_, ch) in Ledger::changes_all(repo.ledger.conn())? {
        if !seen_cs.insert(ch.changeset) {
            continue;
        }
        if !in_export_window(history, head, since, ch.changeset)? {
            continue;
        }
        let row = Ledger::get_changeset(repo.ledger.conn(), ch.changeset)?
            .ok_or_else(|| WispError::integrity("missing changeset"))?;
        counts
            .entry(row.changeset.actor_id)
            .and_modify(|n| *n += 1)
            .or_insert(1);
    }
    let mut out: Vec<ActorIndexRow> = counts
        .into_iter()
        .map(|(id, n)| ActorIndexRow {
            actor_id: id.to_string(),
            display: repo.actor_display(id),
            changeset_count: n,
        })
        .collect();
    out.sort_by(|a, b| a.display.cmp(&b.display));
    Ok(out)
}

fn collect_merges(
    repo: &Repository,
    history: &mut History,
    head: ChangesetId,
    since: Option<ChangesetId>,
) -> Result<Vec<MergeIndexRow>> {
    let mut by_merge: HashMap<ChangesetId, Vec<ChangeId>> = HashMap::new();
    for (merge_cs, change) in Ledger::all_merge_replay_skips(history.conn())? {
        if !in_export_window(history, head, since, merge_cs)? {
            continue;
        }
        by_merge.entry(merge_cs).or_default().push(change);
    }
    let mut out = Vec::new();
    for (merge_cs, skipped) in by_merge {
        let (actor, at) = changeset_meta(repo, merge_cs)?;
        let mut ids: Vec<String> = skipped.into_iter().map(|c| c.to_string()).collect();
        ids.sort();
        out.push(MergeIndexRow {
            merge_changeset_id: merge_cs.to_string(),
            at,
            actor,
            skipped_changes: ids,
        });
    }
    out.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(out)
}

fn collect_removals(
    retractions: &[RetractionRow],
    erasures: &[ErasureMarkerRow],
) -> Vec<RemovalIndexRow> {
    let mut out = Vec::new();
    for r in retractions {
        out.push(RemovalIndexRow {
            kind: "retraction".to_string(),
            target: r.concept_id.clone(),
            reason: r.reason.clone(),
            at: None,
        });
    }
    for e in erasures {
        out.push(RemovalIndexRow {
            kind: "erasure_marker".to_string(),
            target: format!("{}#{}", e.concept_id, e.section_id),
            reason: Some(e.reason.clone()),
            at: None,
        });
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.target.cmp(&b.target)));
    out
}

fn changeset_meta(repo: &Repository, cs: ChangesetId) -> Result<(String, String)> {
    let row = Ledger::get_changeset(repo.ledger.conn(), cs)?
        .ok_or_else(|| WispError::integrity("missing changeset"))?;
    Ok((
        repo.actor_display(row.changeset.actor_id),
        row.changeset.at.clone(),
    ))
}

fn format_inputs(
    repo: &Repository,
    hot: &HotSnap,
    history: &mut History,
    head: ChangesetId,
    ch: &Change,
) -> Result<Vec<InputRow>> {
    let mut out = Vec::new();
    for inp in ch.inputs.iter().take(MAX_INPUTS_PER_VERSION) {
        let (label, state) = match inp {
            InputRef::Concept {
                concept: c,
                section,
                at,
            } => {
                let path = concept_path_at(hot, history, *c, *at)?;
                let label = if let Some(s) = section {
                    let heading = hot
                        .section_headings
                        .get(s)
                        .cloned()
                        .unwrap_or_else(|| s.to_string());
                    format!("{path} § {heading}")
                } else {
                    path
                };
                let state = input_target_state(repo, hot, *c, input_pin_state(history, *at, head));
                (label, state)
            }
            InputRef::Source { concept: c, source } => {
                let path = concept_path_at(hot, history, *c, None)?;
                let state = input_target_state(repo, hot, *c, "source".to_string());
                (format!("{path} (source {source})"), state)
            }
            InputRef::Asset { asset, at } => {
                let label = format!("asset:{asset}");
                (label, input_pin_state(history, *at, head))
            }
        };
        out.push(InputRow { label, state });
    }
    Ok(out)
}

fn input_target_state(
    repo: &Repository,
    hot: &HotSnap,
    concept: ConceptId,
    pin_state: String,
) -> String {
    if hot.retracted.contains_key(&concept) {
        return "retracted".to_string();
    }
    if let Some(scope) = hot.concept_scopes.get(&concept) {
        if scope_erased(repo, *scope).unwrap_or(false) {
            return "erased".to_string();
        }
    }
    pin_state
}

fn concept_path_at(
    hot: &HotSnap,
    history: &mut History,
    concept: ConceptId,
    at: Option<ChangesetId>,
) -> Result<String> {
    if let Some(p) = hot.concept_paths.get(&concept) {
        return Ok(p.clone());
    }
    if let Some(p) = hot.retracted.get(&concept) {
        return Ok(p.clone());
    }
    if let Some(pin) = at {
        if !history.concept_exists_at(concept, pin)? {
            return Ok(format!("{concept} (unresolved)"));
        }
    }
    Ok(concept.to_string())
}

fn input_pin_state(history: &mut History, at: Option<ChangesetId>, head: ChangesetId) -> String {
    match at {
        None => "unpinned".to_string(),
        Some(cs) if history.is_ancestor_or_self(cs, head).unwrap_or(false) => {
            format!("pinned @ {cs}")
        }
        Some(cs) => format!("unresolved pin @ {cs}"),
    }
}

type SectionBodyDiff = (Option<String>, Option<String>, Option<String>, bool);

fn section_version_body_and_diff(
    repo: &Repository,
    history: &mut History,
    ch: &Change,
    skipped_at_head: bool,
) -> Result<SectionBodyDiff> {
    if skipped_at_head {
        return Ok((None, None, None, false));
    }
    if scope_erased(repo, ch.scope)? {
        return Ok((None, None, None, false));
    }
    let body = read_section_body(repo, ch)?;
    let preds = history.predecessors(ch)?;
    let diff = if let Some(pred) = preds.first() {
        if let Ok(Some(old)) = read_section_body(repo, pred) {
            let new_s = body.as_deref().unwrap_or("");
            let d = line_diff(&old, new_s);
            let wd = wisp_html::word_diff(&old, new_s);
            let wd = if wd.is_empty() { None } else { Some(wd) };
            (body, Some(d), wd, false)
        } else {
            (body, None, None, true)
        }
    } else {
        (body, None, None, true)
    };
    Ok(diff)
}

fn scope_erased(repo: &Repository, scope: ScopeId) -> Result<bool> {
    Ok(Ledger::scope(repo.ledger.conn(), scope)?
        .map(|s| s.erased)
        .unwrap_or(false))
}

fn read_section_body(repo: &Repository, ch: &Change) -> Result<Option<String>> {
    if scope_erased(repo, ch.scope)? {
        return Ok(None);
    }
    let pr = ch
        .payload
        .as_ref()
        .ok_or_else(|| WispError::not_found("no payload"))?;
    let pt = repo
        .read_payload(ch.scope, pr)?
        .ok_or_else(|| WispError::key_unavailable("erased"))?;
    let sb = SectionBody::decode(&pt)?;
    Ok(Some(sb.body))
}

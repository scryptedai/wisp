use crate::escape::{html_attr, html_text};
use crate::model::*;
use crate::options::HISTORY_PAGE_SIZE;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use wisp_model::WispError;

pub struct RenderSummary {
    pub page_count: usize,
    pub manifest_sha256: String,
    pub files: Vec<String>,
}

/// Crockford base32 page ids (26 chars) only — never user paths in filenames.
pub fn validate_page_id(id: &str) -> Result<(), WispError> {
    if id.len() != 26
        || !id
            .bytes()
            .all(|b: u8| b.is_ascii_digit() || b.is_ascii_uppercase())
    {
        return Err(WispError::integrity(format!(
            "invalid export page id: {id}"
        )));
    }
    Ok(())
}

pub fn render_site(model: &SiteModel, out_dir: &Path) -> Result<RenderSummary, WispError> {
    fs::create_dir_all(out_dir.join("concepts"))?;
    fs::create_dir_all(out_dir.join("assets"))?;
    fs::create_dir_all(out_dir.join("indexes"))?;

    fs::write(out_dir.join("assets/site.css"), site_css())?;

    let mut page_count = 0usize;

    write_index(model, &out_dir.join("index.html"))?;
    page_count += 1;

    write_activity_index(model, &out_dir.join("indexes/activity.html"))?;
    write_actors_index(model, &out_dir.join("indexes/actors.html"))?;
    write_merges_index(model, &out_dir.join("indexes/merges.html"))?;
    write_removals_index(model, &out_dir.join("indexes/removals.html"))?;
    page_count += 4;

    for c in &model.concepts {
        validate_page_id(&c.id)?;
        let p = out_dir.join("concepts").join(format!("{}.html", c.id));
        write_concept(model, c, &p)?;
        page_count += 1;
    }

    fs::write(out_dir.join("README.txt"), readme_text(model))?;

    let manifest_path = out_dir.join("manifest.json");
    let mut file_hashes: BTreeMap<String, String> = BTreeMap::new();
    collect_hashes(out_dir, out_dir, &mut file_hashes)?;
    let manifest_body = manifest_json(model, &file_hashes);
    fs::write(&manifest_path, &manifest_body)?;
    file_hashes.insert(
        "manifest.json".to_string(),
        sha256_hex(manifest_body.as_bytes()),
    );

    let mut files: Vec<String> = file_hashes.keys().cloned().collect();
    files.sort();
    let manifest_sha256 = file_hashes
        .get("manifest.json")
        .cloned()
        .unwrap_or_default();

    Ok(RenderSummary {
        page_count,
        manifest_sha256,
        files,
    })
}

pub fn render_receipt(
    out_dir: &Path,
    export_id: &str,
    exported_at: &str,
    manifest_sha256: &str,
    signature: Option<&ReceiptSignature>,
) -> Result<(), WispError> {
    let mut body = serde_json::json!({
        "export_id": export_id,
        "exported_at": exported_at,
        "manifest_sha256": manifest_sha256,
        "kind": "html",
    });
    if let Some(sig) = signature {
        body["signature"] = serde_json::json!({
            "actor": sig.actor,
            "algorithm": "ed25519",
            "bytes": sig.bytes_hex,
        });
    }
    let s = serde_json::to_string_pretty(&body).map_err(|e| WispError::internal(e.to_string()))?;
    fs::write(out_dir.join("export-receipt.json"), s)?;
    Ok(())
}

pub struct ReceiptSignature {
    pub actor: String,
    pub bytes_hex: String,
}

struct MergedSection {
    meta: SectionPage,
    versions: Vec<SectionVersion>,
}

fn section_pages(model: &SiteModel) -> &[SectionPage] {
    if !model.section_pages.is_empty() {
        &model.section_pages
    } else {
        &model.sections
    }
}

fn merged_sections_for_concept(model: &SiteModel, concept_id: &str) -> Vec<MergedSection> {
    let mut by_id: BTreeMap<String, MergedSection> = BTreeMap::new();
    for page in section_pages(model) {
        if page.concept_id != concept_id {
            continue;
        }
        let entry = by_id
            .entry(page.id.clone())
            .or_insert_with(|| MergedSection {
                meta: page.clone(),
                versions: Vec::new(),
            });
        entry.versions.extend(page.versions.clone());
        entry.meta.total_versions = page.total_versions;
        entry.meta.versions_truncated = page.versions_truncated || page.page_count > 1;
        entry.meta.page_count = page.page_count;
    }
    let mut out: Vec<MergedSection> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        a.meta
            .heading
            .cmp(&b.meta.heading)
            .then_with(|| a.meta.id.cmp(&b.meta.id))
    });
    out
}

fn rev_fragment(change_id: &str) -> String {
    format!("rev-{change_id}")
}

fn concept_href(concept_id: &str, fragment: Option<&str>) -> String {
    let mut href = format!("concepts/{concept_id}.html");
    if let Some(f) = fragment {
        href.push('#');
        href.push_str(f);
    }
    href
}

fn readme_text(model: &SiteModel) -> String {
    let mut s = format!(
        "wisp HTML export\nref: {}\nhead: {}\ncontent: {}\n",
        model.ref_name,
        model.head.as_deref().unwrap_or("(none)"),
        model.content,
    );
    if model.ids_only {
        s.push_str("ids_only: true\n");
    }
    if let Some(since) = &model.since {
        s.push_str(&format!("since: {since}\n"));
    }
    s.push('\n');
    s.push_str(PLAINTEXT_NOTICE);
    s.push('\n');
    s
}

fn manifest_json(model: &SiteModel, hashes: &BTreeMap<String, String>) -> String {
    let v = serde_json::json!({
        "schema": SCHEMA_VERSION,
        "renderer": RENDERER_VERSION,
        "ref": model.ref_name,
        "head": model.head,
        "content": model.content,
        "ids_only": model.ids_only,
        "since": model.since,
        "limits": {
            "section_versions_per_page": HISTORY_PAGE_SIZE,
            "inputs_per_version": MAX_INPUTS_PER_VERSION,
        },
        "files": hashes,
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

fn collect_hashes(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<(), WispError> {
    for entry in fs::read_dir(dir).map_err(|e| WispError::internal(e.to_string()))? {
        let entry = entry.map_err(|e| WispError::internal(e.to_string()))?;
        let p = entry.path();
        if p.file_name().and_then(|n| n.to_str()) == Some("manifest.json")
            || p.file_name().and_then(|n| n.to_str()) == Some("export-receipt.json")
        {
            continue;
        }
        if p.is_dir() {
            collect_hashes(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            let key = rel.to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&p).map_err(|e| WispError::internal(e.to_string()))?;
            out.insert(key, sha256_hex(&bytes));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let h = Sha256::digest(bytes);
    hex::encode(h)
}

fn page_shell(title: &str, body: &str, css_href: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src 'self' data:; base-uri 'none'; form-action 'none'">
<title>{title}</title>
<link rel="stylesheet" href="{css}">
</head>
<body>
<header class="banner">{notice}</header>
<main>
{body}
</main>
</body>
</html>
"#,
        title = html_text(title),
        css = html_attr(css_href),
        notice = html_text(PLAINTEXT_NOTICE),
        body = body,
    )
}

fn write_index(model: &SiteModel, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str("<h1>wisp export</h1>");
    body.push_str(&format!(
        "<p class=\"wisp-ref-meta\">ref {} · head {}</p>",
        html_text(&model.ref_name),
        html_text(model.head.as_deref().unwrap_or("—"))
    ));
    body.push_str(&format!(
        "<p>{} concepts · {} sections</p>",
        model.overview.concept_count, model.overview.section_count
    ));
    if !model.overview.retractions.is_empty() {
        body.push_str("<h2>Retractions</h2><ul>");
        for r in &model.overview.retractions {
            body.push_str("<li>");
            if validate_page_id(&r.concept_id).is_ok() {
                body.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    html_attr(&concept_href(&r.concept_id, None)),
                    html_text(&r.path)
                ));
            } else {
                body.push_str(&html_text(&r.path));
            }
            if let Some(reason) = &r.reason {
                body.push_str(&format!(
                    " <span class=\"muted\">({})</span>",
                    html_text(reason)
                ));
            }
            body.push_str("</li>");
        }
        body.push_str("</ul>");
    }
    if !model.overview.erasures.is_empty() {
        body.push_str("<h2>Erasure markers</h2><ul>");
        for e in &model.overview.erasures {
            let href = concept_href(&e.concept_id, Some(&e.section_id));
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a> <span class=\"badge badge-erasure\">{}</span></li>",
                html_attr(&href),
                html_text(&e.concept_path),
                html_text(&e.reason)
            ));
        }
        body.push_str("</ul>");
    }
    if let Some(demo) = &model.overview.demo_concept_id {
        body.push_str("<h2>Start here</h2><p><a href=\"");
        body.push_str(&html_attr(&concept_href(demo, None)));
        body.push_str("\">Demo concept</a></p>");
    }
    body.push_str("<h2>Concepts</h2><ul>");
    for c in &model.overview.concepts {
        body.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            html_attr(&concept_href(&c.id, None)),
            html_text(&c.path)
        ));
    }
    body.push_str("</ul>");
    body.push_str(&index_nav("."));
    let html = page_shell("wisp export overview", &body, "assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn write_concept(model: &SiteModel, c: &ConceptPage, path: &Path) -> Result<(), WispError> {
    let sections = merged_sections_for_concept(model, &c.id);
    let mut body = String::new();
    body.push_str("<p><a href=\"../index.html\">Overview</a></p>");
    body.push_str(&format!(
        "<article class=\"wisp-concept\" data-concept-id=\"{}\">",
        html_attr(&c.id)
    ));
    body.push_str(&format!("<h1>{}</h1>", html_text(&c.path)));
    if !c.badges.is_empty() {
        body.push_str("<p class=\"badges\">");
        for b in &c.badges {
            body.push_str(&format!("<span class=\"badge\">{}</span> ", html_text(b)));
        }
        body.push_str("</p>");
    }
    if c.retracted {
        body.push_str("<p class=\"badge\">retracted</p>");
    }

    if sections.is_empty() {
        if let Some(md) = &c.markdown_html {
            body.push_str("<div class=\"wisp-concept-body markdown\">");
            body.push_str(md);
            body.push_str("</div>");
        }
    } else {
        for merged in &sections {
            render_wisp_section(model, &mut body, merged);
        }
    }

    body.push_str("</article>");
    body.push_str(&index_nav(".."));
    let title = c.path.clone();
    let html = page_shell(&title, &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn render_wisp_section(model: &SiteModel, body: &mut String, merged: &MergedSection) {
    let s = &merged.meta;
    validate_page_id(&s.id).ok();
    body.push_str(&format!(
        "<section id=\"{}\" class=\"wisp-section\" data-section-id=\"{}\">",
        html_attr(&s.id),
        html_attr(&s.id)
    ));
    let heading = if s.heading.is_empty() {
        "Section"
    } else {
        s.heading.as_str()
    };
    body.push_str(&format!("<h2>{}</h2>", html_text(heading)));

    if merged.versions.is_empty() {
        body.push_str("<p class=\"muted\">No versions in this export window.</p>");
        body.push_str("</section>");
        return;
    }

    let (current, past): (&[SectionVersion], &[SectionVersion]) = if merged.versions.len() == 1 {
        (&merged.versions[..], &[][..])
    } else {
        (&merged.versions[..1], &merged.versions[1..])
    };

    for v in current {
        render_revision(model, body, v, true);
    }

    if !past.is_empty() {
        body.push_str(&format!(
            "<details class=\"wisp-history\"><summary>Older versions ({})</summary>",
            past.len()
        ));
        for v in past {
            render_revision(model, body, v, false);
        }
        body.push_str("</details>");
    }

    if s.versions_truncated || s.total_versions > merged.versions.len() {
        body.push_str(&format!(
            "<p class=\"muted\">Showing {} of {} versions (export limit {} per slice).</p>",
            merged.versions.len(),
            s.total_versions,
            HISTORY_PAGE_SIZE
        ));
    }

    if let Some(g) = model.graphs.iter().find(|g| g.section_id == s.id) {
        body.push_str("<div class=\"wisp-provenance\" hidden>");
        body.push_str(&g.svg);
        if g.truncated {
            body.push_str("<p class=\"muted\">Provenance graph truncated.</p>");
        }
        body.push_str("</div>");
    }

    body.push_str("</section>");
}

fn render_revision(model: &SiteModel, body: &mut String, v: &SectionVersion, is_head: bool) {
    let skipped = !v.not_kept_at_merge.is_empty();
    let mut classes = String::from("wisp-revision");
    if is_head {
        classes.push_str(" wisp-revision--head");
    }
    if skipped {
        classes.push_str(" wisp-revision--skipped");
    } else if is_head {
        classes.push_str(" wisp-revision--current");
    } else {
        classes.push_str(" wisp-revision--past");
    }

    body.push_str(&format!(
        "<article id=\"{}\" class=\"{}\" data-change-id=\"{}\" data-changeset-id=\"{}\">",
        html_attr(&rev_fragment(&v.change_id)),
        classes,
        html_attr(&v.change_id),
        html_attr(&v.changeset_id)
    ));
    body.push_str("<header class=\"wisp-revision-meta\">");
    body.push_str(&format!(
        "<time datetime=\"{}\">{}</time>",
        html_attr(&v.at),
        html_text(&v.at)
    ));
    body.push_str(&format!(
        " <span class=\"wisp-actor\">{}</span>",
        html_text(&v.actor)
    ));
    if model.content && v.inputs_complete {
        body.push_str(" <span class=\"badge\">inputs complete</span>");
    }
    body.push_str("</header>");

    for m in &v.not_kept_at_merge {
        body.push_str(&format!(
            "<p class=\"merge-skip\">Not kept at merge <a href=\"../indexes/merges.html#merge-{}\">merge</a></p>",
            html_attr(m)
        ));
    }

    if !v.inputs.is_empty() {
        body.push_str("<ul class=\"wisp-inputs\">");
        for inp in &v.inputs {
            body.push_str(&format!(
                "<li data-input-state=\"{}\">{}</li>",
                html_attr(&inp.state),
                html_text(&inp.label)
            ));
        }
        body.push_str("</ul>");
        if v.inputs_truncated {
            body.push_str("<p class=\"muted\">Additional inputs omitted.</p>");
        }
    }

    if model.content && !skipped {
        if let Some(b) = &v.body {
            body.push_str("<div class=\"wisp-body markdown\">");
            body.push_str(&markdown_to_html(b));
            body.push_str("</div>");
        }
        if let Some(diff) = &v.diff_vs_predecessor {
            if !diff.is_empty() {
                body.push_str("<details class=\"wisp-diff\"><summary>Line diff vs predecessor</summary><pre class=\"diff\">");
                body.push_str(&html_text(diff));
                body.push_str("</pre></details>");
            }
        } else if v.diff_missing_baseline {
            body.push_str("<p class=\"muted\">No replay-visible predecessor for diff.</p>");
        }
        if let Some(wd) = &v.word_diff_vs_predecessor {
            if !wd.is_empty() {
                body.push_str("<details class=\"wisp-diff\"><summary>Word diff vs predecessor</summary><pre class=\"diff\">");
                body.push_str(&html_text(wd));
                body.push_str("</pre></details>");
            }
        }
    }

    body.push_str("</article>");
}

pub fn markdown_to_html(md: &str) -> String {
    use crate::sanitize::{sanitize_markdown_events, sanitize_markdown_html};
    use pulldown_cmark::{html, Options, Parser};
    let opts = Options::empty();
    let parser = Parser::new_ext(md, opts);
    let parser = sanitize_markdown_events(parser);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    sanitize_markdown_html(&out)
}

fn index_nav(prefix: &str) -> String {
    format!(
        r#"<nav class="indexes"><h2>Indexes</h2><ul>
<li><a href="{prefix}/indexes/activity.html">Activity</a></li>
<li><a href="{prefix}/indexes/actors.html">Actors</a></li>
<li><a href="{prefix}/indexes/merges.html">Merges</a></li>
<li><a href="{prefix}/indexes/removals.html">Removals</a></li>
</ul></nav>
<script>{site_js}</script>"#,
        prefix = prefix,
        site_js = site_js(),
    )
}

fn activity_href(row: &ActivityRow) -> Option<String> {
    if let Some(c) = &row.concept_id {
        let frag = rev_fragment(&row.change_id);
        return Some(format!("../{}", concept_href(c, Some(&frag))));
    }
    None
}

fn write_activity_index(model: &SiteModel, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str("<p><a href=\"../index.html\">Overview</a></p><h1>Activity</h1>");
    body.push_str("<p class=\"filter-hint\">Filter: <input id=\"wisp-filter\" type=\"search\" placeholder=\"actor, op, target…\" autocomplete=\"off\"></p>");
    body.push_str("<table class=\"index-table\" id=\"wisp-activity\"><thead><tr><th>At</th><th>Actor</th><th>Op</th><th>Target</th></tr></thead><tbody>");
    for row in &model.activity {
        let target_cell = if let Some(href) = activity_href(row) {
            format!(
                "<a href=\"{}\">{}</a>",
                html_attr(&href),
                html_text(&row.target)
            )
        } else {
            html_text(&row.target)
        };
        body.push_str(&format!(
            "<tr data-filter=\"{}\"><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_attr(&format!(
                "{} {} {} {}",
                row.at, row.actor, row.op, row.target
            )),
            html_text(&row.at),
            html_text(&row.actor),
            html_text(&row.op),
            target_cell,
        ));
    }
    body.push_str("</tbody></table>");
    body.push_str(&index_nav(".."));
    let html = page_shell("Activity", &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn write_actors_index(model: &SiteModel, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str("<p><a href=\"../index.html\">Overview</a></p><h1>Actors</h1><ul>");
    for row in &model.actors {
        body.push_str(&format!(
            "<li data-actor-id=\"{}\"><span class=\"wisp-actor\">{}</span> <span class=\"muted\">({} changesets)</span></li>",
            html_attr(&row.actor_id),
            html_text(&row.display),
            row.changeset_count
        ));
    }
    body.push_str("</ul>");
    body.push_str(&index_nav(".."));
    let html = page_shell("Actors", &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn revision_link_for_change(model: &SiteModel, change_id: &str) -> Option<String> {
    for page in section_pages(model) {
        if !page.versions.iter().any(|v| v.change_id == change_id) {
            continue;
        }
        return Some(format!(
            "../{}",
            concept_href(&page.concept_id, Some(&rev_fragment(change_id)))
        ));
    }
    None
}

fn write_merges_index(model: &SiteModel, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str("<p><a href=\"../index.html\">Overview</a></p><h1>Merges</h1>");
    for row in &model.merges {
        body.push_str(&format!(
            "<section id=\"merge-{}\" class=\"wisp-merge\" data-merge-changeset=\"{}\">",
            html_attr(&row.merge_changeset_id),
            html_attr(&row.merge_changeset_id)
        ));
        body.push_str(&format!(
            "<h2><time datetime=\"{}\">{}</time></h2><p class=\"wisp-actor\">{}</p>",
            html_attr(&row.at),
            html_text(&row.at),
            html_text(&row.actor)
        ));
        if !row.skipped_changes.is_empty() {
            body.push_str("<p>Skipped revisions:</p><ul>");
            for ch in &row.skipped_changes {
                if let Some(href) = revision_link_for_change(model, ch) {
                    body.push_str(&format!(
                        "<li><a href=\"{}\">revision</a></li>",
                        html_attr(&href)
                    ));
                } else {
                    body.push_str("<li>revision</li>");
                }
            }
            body.push_str("</ul>");
        }
        body.push_str("</section>");
    }
    body.push_str(&index_nav(".."));
    let html = page_shell("Merges", &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn write_removals_index(model: &SiteModel, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str("<p><a href=\"../index.html\">Overview</a></p><h1>Removals</h1><ul>");
    for row in &model.removals {
        body.push_str(&format!("<li><strong>{}</strong> ", html_text(&row.kind)));
        if let Some(r) = &row.reason {
            body.push_str(&format!("{} ", html_text(r)));
        }
        body.push_str(&format!(
            "<span class=\"muted\">{}</span>",
            html_text(&row.target)
        ));
        if let Some(at) = &row.at {
            body.push_str(&format!(
                " <time datetime=\"{}\">{}</time>",
                html_attr(at),
                html_text(at)
            ));
        }
        body.push_str("</li>");
    }
    body.push_str("</ul>");
    body.push_str(&index_nav(".."));
    let html = page_shell("Removals", &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn site_js() -> &'static str {
    r"document.addEventListener('DOMContentLoaded',()=>{const i=document.getElementById('wisp-filter');const t=document.getElementById('wisp-activity');if(!i||!t)return;const rows=t.querySelectorAll('tbody tr');i.addEventListener('input',()=>{const q=i.value.toLowerCase();rows.forEach(r=>{const f=(r.getAttribute('data-filter')||'').toLowerCase();r.style.display=!q||f.includes(q)?'':'none';});});});"
}

fn site_css() -> &'static str {
    r":root { font-family: system-ui, sans-serif; line-height: 1.5; color: #111; }
body { max-width: 52rem; margin: 0 auto; padding: 1rem; }
.banner { background: #fff3cd; border: 1px solid #e0c36a; padding: 0.75rem; margin-bottom: 1.5rem; font-size: 0.9rem; }
a:focus-visible, summary:focus-visible { outline: 2px solid #005fcc; outline-offset: 2px; }
pre.diff { font-family: ui-monospace, monospace; font-size: 0.85em; background: #f6f8fa; padding: 0.75rem; overflow-x: auto; }
.badge { background: #eee; padding: 0.1rem 0.4rem; border-radius: 3px; font-size: 0.85rem; }
.badge-erasure { background: #fde8e8; }
.muted { color: #555; font-size: 0.9rem; }
.merge-skip { color: #8a1f1f; font-size: 0.9rem; }
.wisp-concept { margin-top: 1rem; }
.wisp-section { margin-top: 2rem; padding-top: 1rem; border-top: 1px solid #e5e5e5; }
.wisp-section:target { outline: 2px solid #005fcc; outline-offset: 4px; }
.wisp-revision { margin: 1rem 0; padding: 0.75rem 0; }
.wisp-revision--current .wisp-body { border-left: 3px solid #005fcc; padding-left: 0.75rem; }
.wisp-revision--skipped { opacity: 0.75; }
.wisp-revision-meta { font-size: 0.9rem; color: #444; margin-bottom: 0.5rem; }
.wisp-actor { font-weight: 500; }
.wisp-inputs { margin: 0.5rem 0; padding-left: 1.25rem; font-size: 0.9rem; }
.wisp-history { margin-top: 0.5rem; }
.wisp-body.markdown h1, .wisp-body.markdown h2, .wisp-body.markdown h3 { margin-top: 0.75em; }
.wisp-ref-meta { font-size: 0.9rem; color: #555; }
.indexes { margin-top: 2rem; border-top: 1px solid #ddd; padding-top: 1rem; }
.index-table { width: 100%; border-collapse: collapse; font-size: 0.9rem; }
.index-table th, .index-table td { border-bottom: 1px solid #eee; padding: 0.35rem 0.5rem; text-align: left; }
.filter-hint input { width: min(100%, 20rem); padding: 0.35rem; }
svg { max-width: 100%; height: auto; }
"
}

use crate::escape::{html_attr, html_text};
use crate::model::*;
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
    fs::create_dir_all(out_dir.join("sections"))?;
    fs::create_dir_all(out_dir.join("assets"))?;

    let css = site_css();
    fs::write(out_dir.join("assets/site.css"), css)?;

    let mut page_count = 0usize;

    write_index(model, &out_dir.join("index.html"))?;
    page_count += 1;

    for c in &model.concepts {
        validate_page_id(&c.id)?;
        let p = out_dir.join("concepts").join(format!("{}.html", c.id));
        write_concept(model, c, &p)?;
        page_count += 1;
    }
    for s in &model.sections {
        validate_page_id(&s.id)?;
        validate_page_id(&s.concept_id)?;
        let p = out_dir.join("sections").join(format!("{}.html", s.id));
        write_section(model, s, &p)?;
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
) -> Result<(), WispError> {
    let body = serde_json::json!({
        "export_id": export_id,
        "exported_at": exported_at,
        "manifest_sha256": manifest_sha256,
        "kind": "html",
    });
    let s = serde_json::to_string_pretty(&body).map_err(|e| WispError::internal(e.to_string()))?;
    fs::write(out_dir.join("export-receipt.json"), s)?;
    Ok(())
}

fn readme_text(model: &SiteModel) -> String {
    format!(
        "wisp HTML export\nref: {}\nhead: {}\ncontent: {}\n\n{}\n",
        model.ref_name,
        model.head.as_deref().unwrap_or("(none)"),
        model.content,
        PLAINTEXT_NOTICE
    )
}

fn manifest_json(model: &SiteModel, hashes: &BTreeMap<String, String>) -> String {
    let v = serde_json::json!({
        "schema": SCHEMA_VERSION,
        "renderer": RENDERER_VERSION,
        "ref": model.ref_name,
        "head": model.head,
        "content": model.content,
        "limits": {
            "section_versions": MAX_SECTION_VERSIONS,
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
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; base-uri 'none'; form-action 'none'">
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
    body.push_str(&format!(
        "<h1>wisp export</h1><p>ref <code>{}</code> · head <code>{}</code></p>",
        html_text(&model.ref_name),
        html_text(model.head.as_deref().unwrap_or("—"))
    ));
    body.push_str(&format!(
        "<p>{} live concepts · {} sections</p>",
        model.overview.concept_count, model.overview.section_count
    ));
    if !model.overview.retractions.is_empty() {
        body.push_str("<h2>Retractions</h2><ul>");
        for r in &model.overview.retractions {
            body.push_str("<li>");
            if validate_page_id(&r.concept_id).is_ok() {
                body.push_str(&format!(
                    "<a href=\"concepts/{}.html\">{}</a>",
                    html_attr(&r.concept_id),
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
            body.push_str(&format!(
                "<li>{} · section <code>{}</code> · {}</li>",
                html_text(&e.concept_path),
                html_text(&e.section_id),
                html_text(&e.reason)
            ));
        }
        body.push_str("</ul>");
    }
    if let Some(demo) = &model.overview.demo_concept_id {
        body.push_str("<h2>Start here</h2><p><a href=\"concepts/");
        body.push_str(&html_attr(demo));
        body.push_str(".html\">Demo concept</a></p>");
    }
    body.push_str("<h2>Concepts</h2><ul>");
    for c in &model.overview.concepts {
        body.push_str(&format!(
            "<li><a href=\"concepts/{}.html\">{}</a></li>",
            html_attr(&c.id),
            html_text(&c.path)
        ));
    }
    body.push_str("</ul>");
    let html = page_shell("wisp export overview", &body, "assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn write_concept(_model: &SiteModel, c: &ConceptPage, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str(&format!(
        "<p><a href=\"../index.html\">Overview</a></p><h1>{}</h1><p><code>{}</code></p>",
        html_text(&c.path),
        html_text(&c.id)
    ));
    if !c.badges.is_empty() {
        body.push_str("<p class=\"badges\">");
        for b in &c.badges {
            body.push_str(&format!("<span class=\"badge\">{}</span> ", html_text(b)));
        }
        body.push_str("</p>");
    }
    if !c.sections.is_empty() {
        body.push_str("<h2>Sections</h2><ul>");
        for s in &c.sections {
            body.push_str(&format!(
                "<li><a href=\"../sections/{}.html\">{}</a> <code>{}</code></li>",
                html_attr(&s.id),
                html_text(&s.heading),
                html_text(&s.id)
            ));
        }
        body.push_str("</ul>");
    }
    if let Some(md) = &c.markdown_html {
        body.push_str("<article class=\"markdown\">");
        body.push_str(md);
        body.push_str("</article>");
    }
    let title = format!("{} — concept", c.path);
    let html = page_shell(&title, &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

fn write_section(model: &SiteModel, s: &SectionPage, path: &Path) -> Result<(), WispError> {
    let mut body = String::new();
    body.push_str(&format!(
        "<p><a href=\"../index.html\">Overview</a> · <a href=\"../concepts/{}.html\">{}</a></p>",
        html_attr(&s.concept_id),
        html_text(&s.concept_path)
    ));
    body.push_str(&format!(
        "<h1>{}</h1><p>Section <code>{}</code></p>",
        html_text(&s.heading),
        html_text(&s.id)
    ));
    body.push_str(&format!(
        "<p>ref <code>{}</code> · head <code>{}</code></p>",
        html_text(&model.ref_name),
        html_text(model.head.as_deref().unwrap_or("—"))
    ));

    for (i, v) in s.versions.iter().enumerate() {
        let cur = i == 0;
        body.push_str(&format!(
            "<section class=\"version{}\"><h2>{}</h2>",
            if cur { " current" } else { "" },
            if cur {
                "Current version".to_string()
            } else {
                format!("Version · {}", html_text(&v.change_id))
            }
        ));
        body.push_str(&format!(
            "<p>Actor: <code>{}</code> · {} · changeset <code>{}</code></p>",
            html_text(&v.actor),
            html_text(&v.at),
            html_text(&v.changeset_id)
        ));
        if model.content && v.inputs_complete {
            body.push_str("<p><strong>Inputs complete</strong> (replacement set)</p>");
        }
        if v.inputs.is_empty() {
            body.push_str("<p>Declared inputs: none</p>");
        } else {
            body.push_str("<h3>Declared inputs</h3><ul>");
            for inp in &v.inputs {
                body.push_str(&format!(
                    "<li>{} <span class=\"muted\">({})</span></li>",
                    html_text(&inp.label),
                    html_text(&inp.state)
                ));
            }
            body.push_str("</ul>");
            if v.inputs_truncated {
                body.push_str("<p class=\"muted\">(inputs truncated)</p>");
            }
        }
        for m in &v.not_kept_at_merge {
            body.push_str(&format!(
                "<p class=\"merge-skip\">Not kept at merge <code>{}</code></p>",
                html_text(m)
            ));
        }
        if model.content {
            if let Some(diff) = &v.diff_vs_predecessor {
                if !diff.is_empty() {
                    body.push_str("<h3>Diff vs predecessor</h3><pre class=\"diff\">");
                    body.push_str(&html_text(diff));
                    body.push_str("</pre>");
                }
            } else if v.diff_missing_baseline {
                body.push_str("<p class=\"muted\">No replay-visible predecessor for diff.</p>");
            }
        }
        if model.content {
            if let Some(b) = &v.body {
                body.push_str("<details><summary>Body</summary><pre class=\"body\">");
                body.push_str(&html_text(b));
                body.push_str("</pre></details>");
            }
        }
        body.push_str("</section>");
    }
    if s.versions_truncated {
        body.push_str(&format!(
            "<p class=\"muted\">Showing {} of {} versions (limit {}).",
            s.versions.len(),
            s.total_versions,
            MAX_SECTION_VERSIONS
        ));
    }
    let title = format!("{} — {}", s.concept_path, s.heading);
    let html = page_shell(&title, &body, "../assets/site.css");
    fs::write(path, html).map_err(|e| WispError::internal(e.to_string()))?;
    Ok(())
}

pub fn markdown_to_html(md: &str) -> String {
    use crate::sanitize::sanitize_markdown_html;
    use pulldown_cmark::{html, Options, Parser};
    let opts = Options::empty();
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    sanitize_markdown_html(&out)
}

fn site_css() -> &'static str {
    r":root { font-family: system-ui, sans-serif; line-height: 1.5; color: #111; }
body { max-width: 52rem; margin: 0 auto; padding: 1rem; }
.banner { background: #fff3cd; border: 1px solid #e0c36a; padding: 0.75rem; margin-bottom: 1.5rem; font-size: 0.9rem; }
a:focus-visible, summary:focus-visible { outline: 2px solid #005fcc; outline-offset: 2px; }
code, pre { font-family: ui-monospace, monospace; font-size: 0.9em; }
pre.diff, pre.body { background: #f6f8fa; padding: 0.75rem; overflow-x: auto; }
.badge { background: #eee; padding: 0.1rem 0.4rem; border-radius: 3px; font-size: 0.85rem; }
.muted { color: #555; font-size: 0.9rem; }
.merge-skip { color: #8a1f1f; }
.version { border-top: 1px solid #ddd; margin-top: 1.5rem; padding-top: 1rem; }
.version.current { border-color: #005fcc; }
.markdown h1, .markdown h2 { margin-top: 1.2em; }
"
}

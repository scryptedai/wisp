//! `wisp export --html` acceptance (Part 1).

mod impact_harness;
mod spy_harness;

use impact_harness::{section, unlock, update, HUMAN};
use serde_json::json;
use spy_harness::*;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;
use wisp_engine::erase::{erase, EraseTarget};
use wisp_engine::html_export::{export_html, verify_html_export};
use wisp_html::HtmlExportOptions;
use wisp_engine::merge::merge_refs;
use wisp_engine::query::{branch, fsck};
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::{catalog, init_and_record_bundle};
use wisp_model::Reason;

fn demo_repo() -> (tempfile::TempDir, Repository) {
    let spec = catalog()
        .bundle("impact-demo")
        .expect("impact-demo bundle")
        .clone();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let repo = init_and_record_bundle(&spec, &root, &unlock()).unwrap();
    (dir, repo)
}

fn site_text(root: &Path) -> String {
    let mut s = String::new();
    for e in WalkDir::new(root) {
        let e = e.unwrap();
        if e.file_type().is_file() {
            s.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
    s
}

#[test]
fn export_html_impact_demo() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("site");
    let v = export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).unwrap();
    assert_eq!(v["kind"], "html");
    assert!(out.join("index.html").is_file());
    assert!(out.join("manifest.json").is_file());
    assert!(out.join("export-receipt.json").is_file());
    let index = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(index.contains("webhook-setup"));
    assert!(index.contains("plaintext copy"));
    fsck(&mut repo).unwrap();

    if std::env::var_os("WRITE_HTML_DEMO").is_some() {
        let demo = Path::new("/tmp/wisp-site-demo");
        let _ = fs::remove_dir_all(demo);
        export_html(&mut repo, "main", demo, HtmlExportOptions::full(true)).unwrap();
        eprintln!("HTML demo: {}", demo.join("index.html").display());
    }
}

#[test]
fn export_html_no_content() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("site-meta");
    export_html(&mut repo, "main", &out, HtmlExportOptions::full(false)).unwrap();
    let concept = fs::read_dir(out.join("concepts"))
        .unwrap()
        .find_map(|e| {
            let e = e.unwrap();
            if e.path().extension().is_some_and(|x| x == "html") {
                Some(e.path())
            } else {
                None
            }
        })
        .expect("concept page");
    let html = fs::read_to_string(concept).unwrap();
    assert!(!html.contains("class=\"wisp-body\""));
    assert!(!html.contains("<pre class=\"diff\">"));
    let all = site_text(&out);
    assert!(!all.contains("HMAC signature"));
}

#[test]
fn export_html_refuses_existing_dir() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("site");
    fs::create_dir(&out).unwrap();
    assert!(export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).is_err());
}

#[test]
fn export_html_refuses_inside_bundle() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("bundle").join("exported");
    assert!(export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).is_err());
}

#[test]
fn export_html_erasure_canary_absent() {
    const WEBHOOK: &str = "notes/webhook-setup.md";
    const NEEDLE: &str = "WISP_HTML_ERASE_CANARY_7f3a9c2e";
    let (dir, mut repo) = demo_repo();
    update(
        &mut repo,
        "main",
        HUMAN,
        WEBHOOK,
        "Setup",
        NEEDLE,
        json!([]),
        json!({}),
    )
    .unwrap();
    let out = dir.path().join("export-site");
    export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).unwrap();
    assert!(site_text(&out).contains(NEEDLE));

    let (sid, _) = section(&mut repo, "main", WEBHOOK, "Setup");
    erase(
        &mut repo,
        "main",
        &EraseTarget::Path(format!("{WEBHOOK}#{sid}")),
        Reason::Policy,
        true,
        false,
        HUMAN,
    )
    .unwrap();

    let out2 = dir.path().join("export-site-2");
    export_html(&mut repo, "main", &out2, HtmlExportOptions::full(true)).unwrap();
    assert!(
        !site_text(&out2).contains(NEEDLE),
        "erased section body must not appear in any export byte"
    );
}

#[test]
fn export_html_merge_skip_marker() {
    let cat = catalog();
    let spec = cat.bundle("warehouse").expect("warehouse bundle");
    let (dir, _root, mut repo) = fresh_spy_repo(spec);
    let branch_name = "task/html-export";
    branch(&mut repo, branch_name, None).unwrap();
    let (ord_sec, ord_ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    update_section_ref(
        &mut repo,
        branch_name,
        "process:branch-bot/1.0",
        "tables/orders",
        ord_sec,
        ord_ver,
        "| col | type |\n|---|---|\n| id | STRING |\n",
        "branch: widen id type",
        vec![],
    )
    .unwrap();
    let (_, ord_ver_main) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:main-bot/1.0",
        "tables/orders",
        ord_sec,
        ord_ver_main,
        "| col | type |\n|---|---|\n| id | INT64 |\n| qty | INT64 |\n",
        "main: add qty column",
        vec![],
    )
    .unwrap();
    let err = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap_err();
    let rows = err.details["conflicts"]["conflicts"].as_array().unwrap();
    let ours = rows[0]["ours"].as_str().unwrap();
    let theirs = rows[0]["theirs"].as_str().unwrap();
    let res_path = dir.path().join("resolutions.json");
    fs::write(
        &res_path,
        format!(r#"{{"resolutions":[{{"ours":"{ours}","theirs":"{theirs}","take":"theirs"}}]}}"#),
    )
    .unwrap();
    merge_refs(&mut repo, branch_name, MAIN_REF, Some(&res_path), false).unwrap();

    let out = dir.path().join("html-export");
    export_html(&mut repo, MAIN_REF, &out, HtmlExportOptions::full(true)).unwrap();
    let all = site_text(&out);
    assert!(
        all.contains("Not kept at merge"),
        "merge replay skip must appear in section history pages"
    );
}

#[test]
fn export_html_semantic_sections() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("site-semantic");
    export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).unwrap();
    assert!(
        !out.join("sections").exists(),
        "per-section HTML files removed; content lives on concept pages"
    );
    let concept = fs::read_dir(out.join("concepts"))
        .unwrap()
        .find_map(|e| {
            let e = e.unwrap();
            if e.path().extension().is_some_and(|x| x == "html") {
                Some(fs::read_to_string(e.path()).unwrap())
            } else {
                None
            }
        })
        .expect("concept html");
    assert!(concept.contains("class=\"wisp-section\""));
    assert!(concept.contains("class=\"wisp-revision"));
    assert!(
        !concept.contains("<code>01"),
        "opaque ids should not be shown as visible code spans on concept pages"
    );
}

#[test]
fn export_html_verify_and_indexes() {
    let (dir, mut repo) = demo_repo();
    let out = dir.path().join("site-verify");
    export_html(&mut repo, "main", &out, HtmlExportOptions::full(true)).unwrap();
    assert!(out.join("indexes/activity.html").is_file());
    assert!(out.join("indexes/merges.html").is_file());
    let v = verify_html_export(&out).unwrap();
    assert_eq!(v["ok"], true);
}

#[test]
fn hostile_markdown_sanitized() {
    let md = "[x](javascript:alert(1))\n\n<script>alert(1)</script>";
    let html = wisp_html::markdown_to_html(md);
    assert!(!html.to_ascii_lowercase().contains("javascript:"));
    assert!(!html.to_ascii_lowercase().contains("<script"));
}

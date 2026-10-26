//! Field search, same-heading siblings on apply, and section version line diff.

use serde_json::json;
use wisp_engine::headless::{apply_direct, ApplyBatch, ApplyOp};
use wisp_engine::query::search_with_params;
use wisp_engine::record::diff_section_versions;
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_engine::search::SearchParams;
use wisp_model::{ChangeId, SectionId};
use wisp_store::Hot;

fn unlock() -> wisp_engine::repo::Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    wisp_engine::repo::Unlock::default()
}

fn seed_blue_node(repo: &mut Repository) -> SectionId {
    apply_direct(
        repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/1".into()),
            message: Some("seed".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: "blue/node.md".into(),
                r#type: Some("concept".into()),
                content: Some("Preamble about Film as a motif in color theory.\n".into()),
                frontmatter: Some(json!({ "title": "blue" })),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .expect("seed");
    let hot = repo.hot(MAIN_REF).unwrap();
    Hot::concept_by_path(hot.conn(), "blue/node")
        .unwrap()
        .unwrap();
    let preamble = Hot::sections_of(hot.conn(), {
        let c = Hot::concept_by_path(hot.conn(), "blue/node")
            .unwrap()
            .unwrap();
        c.concept
    })
    .unwrap()
    .into_iter()
    .find(|s| s.level == 0)
    .expect("preamble");
    preamble.section
}

fn add_heading_section(repo: &mut Repository, heading: &str) -> SectionId {
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), "blue/node")
        .unwrap()
        .unwrap();
    let out = apply_direct(
        repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/1".into()),
            message: Some("add".into()),
            ops: vec![ApplyOp {
                op: "add_section".into(),
                path: "blue/node.md".into(),
                heading: Some(heading.into()),
                level: Some(2),
                content: Some("Body.\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .expect("add_section");
    let sid = out["changes"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|c| c["section"].as_str())
        .expect("section id");
    let _ = c;
    sid.parse().unwrap()
}

fn search_field(
    repo: &mut Repository,
    field: &str,
    query: &str,
    mode: &str,
) -> Vec<serde_json::Value> {
    let params = SearchParams {
        field: Some(field.into()),
        ..Default::default()
    };
    let v = search_with_params(repo, MAIN_REF, query, mode, 20, &params).unwrap();
    v["results"].as_array().cloned().unwrap_or_default()
}

#[test]
fn field_search_and_section_diff_on_blue_node() {
    let dir = tempfile::tempdir().unwrap();
    let mut repo = Repository::init(dir.path(), "0.2", &unlock()).unwrap();
    let _preamble = seed_blue_node(&mut repo);
    let h = "Psychological Effects of Blue";
    let s1 = add_heading_section(&mut repo, h);
    let s2 = add_heading_section(&mut repo, h);
    let s3 = add_heading_section(&mut repo, h);

    let film_heading = search_field(&mut repo, "heading", "Film", "exact");
    assert!(film_heading.is_empty(), "heading Film: {film_heading:?}");

    let peb = search_field(&mut repo, "heading", h, "exact");
    let ids: Vec<SectionId> = peb
        .iter()
        .map(|r| r["section"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&s1) && ids.contains(&s2) && ids.contains(&s3));
    for r in &peb {
        assert_eq!(r["field"].as_str(), Some("heading"));
        assert!(r.get("version").and_then(|v| v.as_str()).is_some());
    }

    let film_body = search_field(&mut repo, "body", "Film", "exact");
    assert_eq!(film_body.len(), 1);
    let quoted_film = search_field(&mut repo, "body", "\"Film\"", "exact");
    assert_eq!(quoted_film.len(), 1, "quoted exact should strip quotes");

    let peb_kw = search_field(&mut repo, "heading", "Psychological", "keyword");
    assert_eq!(peb_kw.len(), 3, "heading keyword: {peb_kw:?}");
    let film_kw = search_field(&mut repo, "body", "Film", "keyword");
    assert_eq!(film_kw.len(), 1, "body keyword: {film_kw:?}");
    assert_eq!(
        film_body[0]["section"].as_str().unwrap(),
        _preamble.to_string()
    );
    let snip = film_body[0]["snippet"].as_str().unwrap_or("");
    assert!(snip.contains("Film"), "snippet: {snip}");

    let title = search_field(&mut repo, "title", "blue", "exact");
    assert_eq!(title.len(), 1);
    assert_eq!(title[0]["path"].as_str(), Some("blue/node"));

    // Fourth add reports the other three as same-heading siblings.
    let out = apply_direct(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/1".into()),
            message: Some("fourth".into()),
            ops: vec![ApplyOp {
                op: "add_section".into(),
                path: "blue/node.md".into(),
                heading: Some(h.into()),
                level: Some(2),
                content: Some("Fourth.\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .unwrap();
    let sibs = out["changes"][0]["same_heading_siblings"]
        .as_array()
        .expect("siblings");
    assert_eq!(sibs.len(), 3);

    // Section line diff: lines present in the draft version but not in the winner.
    let hot = repo.hot(MAIN_REF).unwrap();
    let st = Hot::section(hot.conn(), s1).unwrap().unwrap();
    let base_v: ChangeId = st.version;
    let shared = "Shared film bullets.\n";
    apply_direct(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/1".into()),
            message: Some("normalize".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(s1),
                base_version: Some(base_v.to_string()),
                content: Some(shared.into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let v0 = Hot::section(hot.conn(), s1).unwrap().unwrap().version;
    let draft_body =
        format!("{shared}- Gatsby uses gold; blue is absence.\n- Blue Velvet opens on a lawn.\n");
    let draft_out = apply_direct(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/2".into()),
            message: Some("draft".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(s1),
                base_version: Some(v0.to_string()),
                content: Some(draft_body),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .unwrap();
    let draft_v = draft_out["changes"][0]["version"]
        .as_str()
        .unwrap()
        .parse::<ChangeId>()
        .unwrap();
    let win_out = apply_direct(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("test/1".into()),
            message: Some("winner".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(s1),
                base_version: Some(draft_v.to_string()),
                content: Some(shared.into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
            base: None,
        },
        None,
    )
    .unwrap();
    let winner_v = win_out["changes"][0]["version"]
        .as_str()
        .unwrap()
        .parse::<ChangeId>()
        .unwrap();

    let spec = format!("blue/node#{s1}");
    let diff = diff_section_versions(&mut repo, MAIN_REF, &spec, draft_v, winner_v).unwrap();
    let d = diff["diff"].as_str().unwrap_or("");
    assert!(d.contains("Gatsby"), "diff:\n{d}");
    assert!(d.contains("Blue Velvet"), "diff:\n{d}");
    assert!(
        !d.contains("Shared film"),
        "copied bullets should not appear as new"
    );
}

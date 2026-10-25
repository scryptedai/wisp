//! OKF v0.2 compliance: reserved paths, checkout `sources`, validate findings, log shape.
//! Exercises public `apply`, `validate`, `checkout`, `record`, and `why` — not profile stubs alone.

mod impact_harness;

use impact_harness::*;
use serde_json::{json, Value};
use wisp_engine::checkout::{checkout, render_concept};
use wisp_engine::headless::{apply, ApplyBatch};
use wisp_engine::query;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::MAIN_REF;
use wisp_model::{ChangeId, ConceptId, ExitCode, ScopeId, SectionId};
use wisp_profile::Level;
use wisp_store::{ConceptState, Hot, SectionState};

const NOTE: &str = "---\ntype: Observation\n---\n## Note\n\nsqueak\n";
const DOOR: &str = "---\ntype: Table\n---\n## Behavior\n\nopens\n";

fn findings_at(repo: &mut wisp_engine::repo::Repository, concept_path: &str) -> Vec<Value> {
    let v = match query::validate(repo, MAIN_REF, Some(concept_path), false) {
        Ok(v) => v,
        Err(e) if e.code == ExitCode::Validation => e.details["findings"].clone(),
        Err(e) => panic!("validate: {e}"),
    };
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["path"].as_str() == Some(concept_path))
        .cloned()
        .collect()
}

fn has_finding(findings: &[Value], code: &str, level: Level) -> bool {
    let lvl = match level {
        Level::Must => "must",
        Level::Should => "should",
    };
    findings
        .iter()
        .any(|f| f["code"].as_str() == Some(code) && f["level"].as_str() == Some(lvl))
}

fn parsed_fm(text: &str) -> wisp_parse::Frontmatter {
    wisp_parse::frontmatter::split(text).unwrap().0
}

fn open_hot_file(repo: &wisp_engine::repo::Repository) -> wisp_store::Hot {
    Hot::open(&repo.hot_path(MAIN_REF)).expect("open hot db")
}

fn seed_illegal_concept_at_index(repo: &mut wisp_engine::repo::Repository) -> ConceptId {
    let cid = ConceptId::new();
    let scope = ScopeId::new();
    repo.new_scope_key(scope).expect("scope key");
    let v = ChangeId::new();
    let sscope = ScopeId::new();
    repo.new_scope_key(sscope).expect("section scope");
    let sec = SectionId::new();
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(repo);
    Hot::upsert_concept(
        hot.conn(),
        &ConceptState {
            concept: cid,
            path: "index".into(),
            type_: Some("topic".into()),
            status: None,
            scope,
            version: v,
            gen_actor: None,
            gen_at: Some("2026-10-25T12:00:00Z".into()),
            content_version: Some(v),
        },
    )
    .unwrap();
    Hot::upsert_section(
        hot.conn(),
        &SectionState {
            concept: cid,
            section: sec,
            idx: 0,
            level: 0,
            heading: String::new(),
            heading_line: String::new(),
            body: "prose only\n".into(),
            scope: sscope,
            version: v,
        },
    )
    .unwrap();
    drop(hot);
    cid
}

// --- 1. Reserved concept paths ---

#[test]
fn reserved_create_concept_refused_and_no_hot_row() {
    let (_d, _root, mut repo) = fresh(&[("observations/note.md", NOTE)]);
    for path in ["index", "index.md", "topics/log", "topics/log.md"] {
        let batch: ApplyBatch = serde_json::from_value(json!({
            "actor": HUMAN,
            "ops": [{ "op": "create_concept", "path": path, "type": "topic" }]
        }))
        .unwrap();
        let err = apply(&mut repo, batch, None).unwrap_err();
        assert_eq!(err.code, ExitCode::Refused, "path={path}");
        assert!(
            err.message.contains("reserved") || err.message.contains("§3.1"),
            "{err}"
        );
    }
    {
        let hot = open_hot_file(&repo);
        assert!(
            Hot::concept_by_path(hot.conn(), "index").unwrap().is_none(),
            "index concept must not exist"
        );
        assert!(Hot::concept_by_path(hot.conn(), "topics/log")
            .unwrap()
            .is_none());
    }
    let ok: ApplyBatch = serde_json::from_value(json!({
        "actor": HUMAN,
        "ops": [{ "op": "create_concept", "path": "meta/index-notes.md", "type": "topic" }]
    }))
    .unwrap();
    apply(&mut repo, ok, None).unwrap();
    let hot = open_hot_file(&repo);
    assert!(Hot::concept_by_path(hot.conn(), "meta/index-notes")
        .unwrap()
        .is_some());
}

#[test]
fn reserved_concept_validate_must_via_query() {
    let (_d, _root, mut repo) = fresh(&[("observations/note.md", NOTE)]);
    seed_illegal_concept_at_index(&mut repo);
    let f = findings_at(&mut repo, "index");
    assert!(
        has_finding(&f, "reserved_concept", Level::Must),
        "findings: {f:?}"
    );
}

// --- 2. Declared inputs → checkout `sources` ---

#[test]
fn checkout_frontmatter_lists_projected_input_pin() {
    let (_d, root, mut repo) = fresh(&[("tables/door.md", DOOR), ("observations/note.md", NOTE)]);
    let (door_sid, _) = section(&mut repo, MAIN_REF, "tables/door.md", "Behavior");
    let (note_sid, _) = section(&mut repo, MAIN_REF, "observations/note.md", "Note");
    update(
        &mut repo,
        MAIN_REF,
        AGENT,
        "tables/door.md",
        "Behavior",
        "opens loudly",
        json!([{
            "concept": "observations/note.md",
            "section": note_sid.to_string()
        }]),
        json!({}),
    )
    .unwrap();
    repo.flush_materialize().unwrap();
    checkout(&mut repo, MAIN_REF, Some(&root), false, false).unwrap();
    let text = std::fs::read_to_string(root.join("tables/door.md")).unwrap();
    let fm = parsed_fm(&text);
    let sources = fm.get("sources").expect("sources in checkout");
    assert!(
        sources.contains("resource: /observations/note.md"),
        "{sources}"
    );
    assert!(sources.contains(&note_sid.to_string()));
    assert!(sources.contains("title: Note"));
    let hot = repo.hot(MAIN_REF).unwrap();
    let door = Hot::concept_by_path(hot.conn(), "tables/door")
        .unwrap()
        .unwrap();
    let cv = door.content_version.expect("content version");
    let inputs = ledger_inputs(&repo, cv);
    assert_eq!(inputs.len(), 1);
    let why = query::why(
        &mut repo,
        MAIN_REF,
        &format!("tables/door.md#{door_sid}"),
        2,
    )
    .unwrap();
    assert!(
        !why["history"]["inputs"].as_array().unwrap().is_empty(),
        "why still walks ledger inputs: {why}"
    );
}

#[test]
fn checkout_sources_merge_hand_author_without_clobber() {
    let (_d, _root, mut repo) = fresh(&[("tables/door.md", DOOR), ("observations/note.md", NOTE)]);
    let (note_sid, _) = section(&mut repo, MAIN_REF, "observations/note.md", "Note");
    let resource = "/observations/note.md";
    apply_json(
        &mut repo,
        json!({
            "actor": HUMAN,
            "ops": [{
                "op": "source_put",
                "path": "tables/door.md",
                "key": "hand-1",
                "value": {
                    "id": "hand-1",
                    "resource": resource,
                    "author": "human:tim",
                    "title": "Old title"
                }
            }]
        }),
    )
    .unwrap();
    update(
        &mut repo,
        MAIN_REF,
        AGENT,
        "tables/door.md",
        "Behavior",
        "opens",
        json!([{ "concept": "observations/note.md", "section": note_sid.to_string() }]),
        json!({}),
    )
    .unwrap();
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(&repo);
    let door = Hot::concept_by_path(hot.conn(), "tables/door")
        .unwrap()
        .unwrap();
    let view = wisp_engine::checkout::concept_view(&repo, hot.conn(), &door, None, None).unwrap();
    assert_eq!(view.sources.len(), 1);
    let s = &view.sources[0];
    assert_eq!(s["author"], "human:tim");
    assert_eq!(s["resource"], resource);
    assert_eq!(s["title"], "Old title");
    assert_eq!(s["id"], "hand-1");
    assert_eq!(s["section"], note_sid.to_string());
}

#[test]
fn checkout_sources_record_roundtrip_persists_projection() {
    let (_d, root, mut repo) = fresh(&[("tables/door.md", DOOR), ("observations/note.md", NOTE)]);
    let (note_sid, _) = section(&mut repo, MAIN_REF, "observations/note.md", "Note");
    update(
        &mut repo,
        MAIN_REF,
        AGENT,
        "tables/door.md",
        "Behavior",
        "opens",
        json!([{ "concept": "observations/note.md", "section": note_sid.to_string() }]),
        json!({}),
    )
    .unwrap();
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(&repo);
    let door = Hot::concept_by_path(hot.conn(), "tables/door")
        .unwrap()
        .unwrap();
    let rendered = render_concept(&repo, hot.conn(), &door, false, None, None).unwrap();
    std::fs::write(root.join("tables/door.md"), rendered).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            message: Some("adopt projected sources".into()),
            ..Default::default()
        },
    )
    .unwrap();
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(&repo);
    let door = Hot::concept_by_path(hot.conn(), "tables/door")
        .unwrap()
        .unwrap();
    let view2 = wisp_engine::checkout::concept_view(&repo, hot.conn(), &door, None, None).unwrap();
    assert_eq!(view2.sources.len(), 1);
    assert_eq!(view2.sources[0]["resource"], "/observations/note.md");
    assert_eq!(view2.sources[0]["id"], note_sid.to_string());
}

#[test]
fn checkout_skips_unresolvable_input_pin() {
    let (_d, _root, mut repo) = fresh(&[("tables/door.md", DOOR)]);
    let (door_sid, door_ver) = section(&mut repo, MAIN_REF, "tables/door.md", "Behavior");
    let ghost = SectionId::new();
    let batch: ApplyBatch = serde_json::from_value(json!({
        "actor": AGENT,
        "ops": [{
            "op": "update_section",
            "path": "tables/door.md",
            "section": door_sid.to_string(),
            "base_version": door_ver.to_string(),
            "content": "x"
        }],
        "inputs": [{ "concept": "tables/door.md", "section": ghost.to_string() }]
    }))
    .unwrap();
    apply(&mut repo, batch, None).unwrap();
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(&repo);
    let door = Hot::concept_by_path(hot.conn(), "tables/door")
        .unwrap()
        .unwrap();
    let view = wisp_engine::checkout::concept_view(&repo, hot.conn(), &door, None, None).unwrap();
    assert!(view.sources.is_empty());
}

// --- 3. Timestamps and stale ---

#[test]
fn validate_stale_after_bad_timestamp_format() {
    let (_d, _root, mut repo) = fresh(&[("topics/a.md", "---\ntype: topic\n---\n## P\n\nx\n")]);
    apply_json(
        &mut repo,
        json!({
            "actor": HUMAN,
            "ops": [{
                "op": "set_key", "path": "topics/a.md", "key": "stale_after",
                "base_version": "absent", "value": "2026-09-23"
            }]
        }),
    )
    .unwrap();
    let f = findings_at(&mut repo, "topics/a");
    assert!(has_finding(&f, "timestamp_format", Level::Should));
}

#[test]
fn validate_stale_after_past_is_stale() {
    let (_d, _root, mut repo) = fresh(&[(
        "topics/a.md",
        "---\ntype: topic\nstale_after: 2026-09-23T00:00:00Z\n---\n## P\n\nx\n",
    )]);
    record(
        &mut repo,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            message: Some("index stale".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let f = findings_at(&mut repo, "topics/a");
    assert!(has_finding(&f, "stale", Level::Should));
}

#[test]
fn validate_stale_after_future_not_stale() {
    let (_d, _root, mut repo) = fresh(&[(
        "topics/a.md",
        "---\ntype: topic\nstale_after: 2099-01-01T00:00:00Z\n---\n## P\n\nx\n",
    )]);
    record(
        &mut repo,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            ..Default::default()
        },
    )
    .unwrap();
    let f = findings_at(&mut repo, "topics/a");
    assert!(!has_finding(&f, "stale", Level::Should));
}

#[test]
fn validate_generated_at_must_be_rfc3339() {
    let (_d, _root, mut repo) = fresh(&[("observations/note.md", NOTE)]);
    repo.drop_hot(MAIN_REF);
    let hot = open_hot_file(&repo);
    let template = Hot::concept_by_path(hot.conn(), "observations/note")
        .unwrap()
        .unwrap();
    let cid = ConceptId::new();
    let scope = ScopeId::new();
    repo.new_scope_key(scope).unwrap();
    let v = ChangeId::new();
    Hot::upsert_concept(
        hot.conn(),
        &ConceptState {
            concept: cid,
            path: "topics/bad-time".into(),
            type_: Some("topic".into()),
            status: None,
            scope,
            version: v,
            gen_actor: template.gen_actor,
            gen_at: Some("not-a-timestamp".into()),
            content_version: Some(v),
        },
    )
    .unwrap();
    drop(hot);
    let f = findings_at(&mut repo, "topics/bad-time");
    assert!(
        f.iter().any(|x| x["code"] == "timestamp_format"
            && x["message"].as_str().unwrap().contains("generated.at")),
        "{f:?}"
    );
}

// --- 4. Frontmatter shadow ---

#[test]
fn validate_frontmatter_shadow_and_negatives() {
    let (_d, _root, mut repo) = fresh(&[(
        "topics/child.md",
        "---\ntype: topic\nkind: unwritten\ntitle: Child\n---\nkind: branch\n\n# Child\n",
    )]);
    record(
        &mut repo,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            message: Some("re-index".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let f = findings_at(&mut repo, "topics/child");
    assert!(has_finding(&f, "frontmatter_shadow", Level::Should));
    assert!(
        f.iter()
            .filter(|x| x["code"] == "frontmatter_shadow")
            .all(|x| x["message"].as_str().unwrap().contains("kind")),
        "{f:?}"
    );
    assert!(
        !f.iter()
            .any(|x| x["message"].as_str().unwrap_or("").contains("title")),
        "matching title should not shadow"
    );
    let (_d2, _root2, mut repo2) = fresh(&[(
        "topics/plain.md",
        "---\ntype: topic\n---\nOnly prose here.\n",
    )]);
    record(
        &mut repo2,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            ..Default::default()
        },
    )
    .unwrap();
    let f2 = findings_at(&mut repo2, "topics/plain");
    assert!(!has_finding(&f2, "frontmatter_shadow", Level::Should));
}

// --- 5. Log action word ---

#[test]
fn checkout_log_uses_bold_action_word() {
    let (_d, root, mut repo) = fresh(&[("tables/orders.md", DOOR)]);
    checkout(&mut repo, MAIN_REF, Some(&root), false, false).unwrap();
    let log = std::fs::read_to_string(root.join("log.md")).unwrap();
    assert!(log.contains("**Creation**"), "{log}");
    assert!(log.contains("by "), "{log}");
    assert!(
        log.contains("tables/orders.md") || log.contains("orders.md"),
        "{log}"
    );
}

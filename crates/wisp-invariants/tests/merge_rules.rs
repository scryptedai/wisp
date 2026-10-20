//! §11.5 rule battery: one integration test per `wisp_merge::RULES` row (see contrib/merge-rules/cases.toml).

mod spy_harness;

use spy_harness::*;
use std::fs;
use std::path::{Path, PathBuf};
use wisp_engine::headless::{apply, ApplyBatch, ApplyOp};
use wisp_engine::merge::merge_refs;
use wisp_engine::query::{branch, mv};
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::catalog;
use wisp_merge::RULES;
use wisp_model::{ExitCode, SectionId};
use wisp_store::Hot;
fn warehouse() -> (tempfile::TempDir, Repository) {
    let cat = catalog();
    let spec = cat.bundle("warehouse").expect("warehouse");
    let (dir, _root, repo) = fresh_spy_repo(spec);
    (dir, repo)
}

fn apply_one(repo: &mut Repository, ref_name: &str, actor: &str, message: &str, op: ApplyOp) {
    apply(
        repo,
        ApplyBatch {
            r#ref: Some(ref_name.into()),
            base: None,
            actor: Some(actor.into()),
            message: Some(message.into()),
            ops: vec![op],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
        },
        None,
    )
    .unwrap();
}

fn cases_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrib/merge-rules/cases.toml")
}

fn load_cases() -> toml::Table {
    let raw = fs::read_to_string(cases_path()).unwrap();
    raw.parse().unwrap()
}

#[test]
fn merge_rules_table_matches_battery() {
    let table = load_cases();
    let cases = table["case"].as_array().expect("case array");
    assert_eq!(cases.len(), RULES.len(), "one battery case per RULES row");
    for (rule_row, _expect) in RULES {
        let matches = cases
            .iter()
            .filter(|c| c.get("rule").and_then(|v| v.as_str()) == Some(*rule_row))
            .count();
        assert_eq!(matches, 1, "exactly one case per RULES row: {rule_row}");
    }
}

fn section_headings(repo: &mut Repository, ref_name: &str, concept_path: &str) -> Vec<String> {
    let hot = repo.hot(ref_name).unwrap();
    let c = Hot::concept_by_path(hot.conn(), concept_path)
        .unwrap()
        .unwrap_or_else(|| panic!("no concept {concept_path}"));
    Hot::sections_of(hot.conn(), c.concept)
        .unwrap()
        .into_iter()
        .map(|s| s.heading)
        .collect()
}

fn add_section_after_ref(
    repo: &mut Repository,
    ref_name: &str,
    actor: &str,
    concept_path: &str,
    after: SectionId,
    heading: &str,
    body: &str,
    message: &str,
) {
    apply_one(
        repo,
        ref_name,
        actor,
        message,
        ApplyOp {
            op: "add_section".into(),
            path: format!("{concept_path}.md"),
            section: None,
            base_version: None,
            heading: Some(heading.into()),
            level: Some(2),
            content: Some(format!("{body}\n")),
            after: Some(after.to_string()),
            key: None,
            value: None,
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    );
}

#[test]
fn rule_same_section_same_commitment() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/same-commit";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    let body = "| col | type |\n|---|---|\n| id | INT64 |\n";
    update_section_ref(
        &mut repo,
        branch_name,
        "process:b/1",
        "tables/orders",
        sec,
        ver,
        body,
        "branch",
        vec![],
    )
    .unwrap();
    let (_, ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/1",
        "tables/orders",
        sec,
        ver_m,
        body,
        "main",
        vec![],
    )
    .unwrap();
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

#[test]
fn rule_resolution_skips_stacked_branch_edits() {
    let (dir, mut repo) = warehouse();
    let branch_name = "rule/stacked-res";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    update_section_ref(
        &mut repo,
        branch_name,
        "process:b/1",
        "tables/orders",
        sec,
        ver,
        "| col | type |\n|---|---|\n| id | INT64 |\n| v1 | STRING |\n",
        "branch v1",
        vec![],
    )
    .unwrap();
    let (_, ver2) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    update_section_ref(
        &mut repo,
        branch_name,
        "process:b/1",
        "tables/orders",
        sec,
        ver2,
        "| col | type |\n|---|---|\n| id | INT64 |\n| v2 | STRING |\n",
        "branch v2",
        vec![],
    )
    .unwrap();
    let (_, ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/1",
        "tables/orders",
        sec,
        ver_m,
        "| col | type |\n|---|---|\n| id | INT64 |\n| main-final | STRING |\n",
        "main",
        vec![],
    )
    .unwrap();
    let err = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    let envelope = &err.details["conflicts"];
    let rows = envelope["conflicts"].as_array().expect("conflicts array");
    let row = &rows[0];
    let ours = row["ours"].as_str().unwrap();
    let theirs = row["theirs"].as_str().unwrap();
    assert!(
        row["source_changes"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "conflict should list source-side change chain"
    );
    let res_path = dir.path().join("resolutions.json");
    std::fs::write(
        &res_path,
        format!(r#"{{"resolutions":[{{"ours":"{ours}","theirs":"{theirs}","take":"ours"}}]}}"#),
    )
    .unwrap();
    merge_refs(&mut repo, branch_name, MAIN_REF, Some(&res_path), false).unwrap();
    let body = {
        let hot = repo.hot(MAIN_REF).unwrap();
        wisp_store::Hot::section(hot.conn(), sec)
            .unwrap()
            .expect("section")
            .body
    };
    assert!(body.contains("main-final"));
    assert!(!body.contains("| v1 |"));
    assert!(!body.contains("| v2 |"));
    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
    let (_, ver3) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/2",
        "tables/orders",
        sec,
        ver3,
        "| col | type |\n|---|---|\n| id | INT64 |\n| post-merge | STRING |\n",
        "after resolved merge",
        vec![],
    )
    .unwrap();
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

#[test]
fn rule_same_section_different_content() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/diff";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    update_section_ref(
        &mut repo,
        branch_name,
        "process:b/1",
        "tables/orders",
        sec,
        ver,
        "| col | type |\n|---|---|\n| id | STRING |\n",
        "branch",
        vec![],
    )
    .unwrap();
    let (_, ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/1",
        "tables/orders",
        sec,
        ver_m,
        "| col | type |\n|---|---|\n| id | INT64 |\n| q | INT64 |\n",
        "main",
        vec![],
    )
    .unwrap();
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_remove_vs_update() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/remove";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    apply_one(
        &mut repo,
        branch_name,
        "process:b/1",
        "branch remove",
        ApplyOp {
            op: "remove_section".into(),
            path: "tables/orders".into(),
            section: Some(sec),
            base_version: Some(ver.to_string()),
            heading: None,
            level: None,
            content: None,
            after: None,
            key: None,
            value: None,
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    );
    let (_, ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/1",
        "tables/orders",
        sec,
        ver_m,
        "| col | type |\n|---|---|\n| id | INT64 |\n| extra | INT64 |\n",
        "main",
        vec![],
    )
    .unwrap();
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_frontmatter_same_commitment() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/fm-same";
    branch(&mut repo, branch_name, None).unwrap();
    for (r, actor) in [(branch_name, "process:b/1"), (MAIN_REF, "process:m/1")] {
        apply_one(
            &mut repo,
            r,
            actor,
            "title",
            ApplyOp {
                op: "set_key".into(),
                path: "tables/orders".into(),
                section: None,
                base_version: None,
                heading: None,
                level: None,
                content: None,
                after: None,
                key: Some("title".into()),
                value: Some(serde_json::json!("Orders Table")),
                r#type: None,
                frontmatter: None,
                note: None,
                split_from: None,
                merged_from: None,
            },
        );
    }
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
}

#[test]
fn rule_frontmatter_different() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/fm-diff";
    branch(&mut repo, branch_name, None).unwrap();
    apply_one(
        &mut repo,
        branch_name,
        "process:b/1",
        "title b",
        ApplyOp {
            op: "set_key".into(),
            path: "tables/orders".into(),
            section: None,
            base_version: None,
            heading: None,
            level: None,
            content: None,
            after: None,
            key: Some("title".into()),
            value: Some(serde_json::json!("Branch Title")),
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    );
    apply_one(
        &mut repo,
        MAIN_REF,
        "process:m/1",
        "title m",
        ApplyOp {
            op: "set_key".into(),
            path: "tables/orders".into(),
            section: None,
            base_version: None,
            heading: None,
            level: None,
            content: None,
            after: None,
            key: Some("title".into()),
            value: Some(serde_json::json!("Main Title")),
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    );
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_verify_union() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/verify";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, _) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    for (r, actor) in [(branch_name, "human:alice"), (MAIN_REF, "human:bob")] {
        apply_one(
            &mut repo,
            r,
            actor,
            "verify",
            ApplyOp {
                op: "verify".into(),
                path: "tables/orders".into(),
                section: Some(sec),
                base_version: None,
                heading: None,
                level: None,
                content: None,
                after: None,
                key: None,
                value: None,
                r#type: None,
                frontmatter: None,
                note: None,
                split_from: None,
                merged_from: None,
            },
        );
    }
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
}

#[test]
fn rule_concept_path_move_conflict() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/move";
    branch(&mut repo, branch_name, None).unwrap();
    mv(
        &mut repo,
        branch_name,
        "tables/orders.md",
        "tables/orders-archive.md",
        "human:branch",
    )
    .unwrap();
    mv(
        &mut repo,
        MAIN_REF,
        "tables/orders.md",
        "analytics/orders.md",
        "human:main",
    )
    .unwrap();
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_two_concepts_same_path_record() {
    let cat = catalog();
    let spec = cat.bundle("okf-minimal").expect("okf-minimal");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let err = apply(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("human:test".into()),
            message: Some("duplicate path".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: "notes/hello.md".into(),
                section: None,
                base_version: None,
                heading: None,
                level: None,
                content: None,
                after: None,
                key: None,
                value: None,
                r#type: Some("Note".into()),
                frontmatter: None,
                note: None,
                split_from: None,
                merged_from: None,
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
        },
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    let detail = err.details.to_string();
    assert!(
        detail.contains("path_conflict") || detail.contains("notes/hello"),
        "{err:?}"
    );
}

#[test]
fn rule_section_order_layered() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/order";
    branch(&mut repo, branch_name, None).unwrap();
    let (schema, _) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    add_section_after_ref(
        &mut repo,
        MAIN_REF,
        "process:m/1",
        "tables/orders",
        schema,
        "Main tail",
        "added on main after schema",
        "main add",
    );
    add_section_after_ref(
        &mut repo,
        branch_name,
        "process:b/1",
        "tables/orders",
        schema,
        "Branch tail",
        "added on branch after schema",
        "branch add",
    );
    merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
    // §11.5: keep into order, insert source-only sections after their nearest preceding matched anchor.
    let headings = section_headings(&mut repo, MAIN_REF, "tables/orders");
    assert_eq!(
        headings,
        vec![
            String::new(),
            "Schema".into(),
            "Branch tail".into(),
            "Main tail".into(),
            "Behavior".into(),
        ],
        "merged section order"
    );
}

#[test]
fn rule_retract_vs_update() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/retract";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, _ver) = section_at_ref(&mut repo, branch_name, "tables/orders", "Schema");
    wisp_engine::erase::retract(
        &mut repo,
        branch_name,
        "tables/orders",
        wisp_model::Reason::Withdrawn,
        None,
        "human:alice",
    )
    .unwrap();
    let (_, ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:m/1",
        "tables/orders",
        sec,
        ver_m,
        "edited on main\n",
        "main",
        vec![],
    )
    .unwrap();
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_set_status_conflict() {
    let (_dir, mut repo) = warehouse();
    let branch_name = "rule/status";
    branch(&mut repo, branch_name, None).unwrap();
    for (r, actor, status) in [
        (branch_name, "process:b/1", "draft"),
        (MAIN_REF, "process:m/1", "deprecated"),
    ] {
        apply_one(
            &mut repo,
            r,
            actor,
            "status",
            ApplyOp {
                op: "set_status".into(),
                path: "tables/orders".into(),
                section: None,
                base_version: None,
                heading: None,
                level: None,
                content: None,
                after: None,
                key: None,
                value: Some(serde_json::json!(status)),
                r#type: None,
                frontmatter: None,
                note: None,
                split_from: None,
                merged_from: None,
            },
        );
    }
    assert_eq!(
        merge_refs(&mut repo, branch_name, MAIN_REF, None, false)
            .unwrap_err()
            .code,
        ExitCode::Conflict
    );
}

#[test]
fn rule_erase_wins() {
    let cat = catalog();
    let spec = cat.bundle("okf-minimal").expect("okf-minimal");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let branch_name = "rule/erase";
    branch(&mut repo, branch_name, None).unwrap();
    let (sec, ver) = section_at(&mut repo, "notes/hello", "Detail");
    update_section(
        &mut repo,
        "process:m/1",
        "notes/hello",
        sec,
        ver,
        "main edit while branch will erase\n",
        "main",
        vec![],
    )
    .unwrap();
    wisp_engine::erase::erase(
        &mut repo,
        branch_name,
        &wisp_engine::erase::EraseTarget::Path("notes/hello".into()),
        wisp_model::Reason::Policy,
        true,
        "human:admin",
    )
    .unwrap();
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
    let wins = merged["erase_wins"].as_array().unwrap();
    assert!(!wins.is_empty());
    assert_eq!(wins[0]["reason"], "erased_on_source");
    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

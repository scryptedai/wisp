//! Ref merge contracts (§11.5): fast-forward, disjoint replay, scope conflicts.
//!
//! Scenario metadata: `wisp-corpus/data/scenarios/merge/*/scenario.toml`
//! Narrative: `wisp-corpus/data/scenarios/merge/README.md`

mod spy_harness;

use spy_harness::*;
use wisp_corpus::load_merge_scenario;
use wisp_engine::merge::merge_refs;
use wisp_engine::query::branch;
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::catalog;
use wisp_model::{ExitCode, SectionId};
use wisp_store::Hot;

fn warehouse_repo() -> (tempfile::TempDir, Repository) {
    let cat = catalog();
    let spec = cat.bundle("warehouse").expect("warehouse bundle");
    let (dir, _root, repo) = fresh_spy_repo(spec);
    (dir, repo)
}

fn section_body(repo: &mut Repository, ref_name: &str, section: SectionId) -> String {
    let hot = repo.hot(ref_name).unwrap();
    Hot::section(hot.conn(), section)
        .unwrap()
        .expect("section")
        .body
}

#[test]
fn merge_disjoint_sections_fast_forward_and_replay() {
    let cat = catalog();
    let meta = load_merge_scenario(
        cat.scenario("merge.disjoint-sections")
            .expect("merge.disjoint-sections"),
    )
    .expect("scenario.toml");
    assert_eq!(meta.bundle, "warehouse");
    assert_eq!(meta.expect_outcome, "merged");

    // Fast-forward when only the branch commits.
    let (_dir, mut repo) = warehouse_repo();
    let branch_name = "task/ff-only";
    branch(&mut repo, branch_name, None).unwrap();
    let (cust_sec, cust_ver) = section_at_ref(&mut repo, branch_name, "tables/customers", "Schema");
    update_section_ref(
        &mut repo,
        branch_name,
        "process:branch-bot/1.0",
        "tables/customers",
        cust_sec,
        cust_ver,
        "customer_id, name, region\n",
        "branch: widen customers schema",
        vec![],
    )
    .unwrap();
    let ff = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(ff["status"], "fast_forward");
    assert!(section_body(&mut repo, MAIN_REF, cust_sec).contains("region"));

    // Disjoint replay when main and branch each touch different sections.
    let (_dir2, mut repo2) = warehouse_repo();
    let branch_name = &meta.branch;
    branch(&mut repo2, branch_name, None).unwrap();
    let (cust_sec, cust_ver) =
        section_at_ref(&mut repo2, branch_name, "tables/customers", "Schema");
    update_section_ref(
        &mut repo2,
        branch_name,
        "process:branch-bot/1.0",
        "tables/customers",
        cust_sec,
        cust_ver,
        "customer_id, name, tier\n",
        "branch: add tier column",
        vec![],
    )
    .unwrap();
    let (ord_sec, ord_ver) = section_at(&mut repo2, "tables/orders", "Schema");
    update_section(
        &mut repo2,
        "process:main-bot/1.0",
        "tables/orders",
        ord_sec,
        ord_ver,
        "| col | type |\n|---|---|\n| id | INT64 |\n| ts | TIMESTAMP |\n",
        "main: add ts column",
        vec![],
    )
    .unwrap();
    let merged = merge_refs(&mut repo2, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
    assert!(merged["changesets_replayed"].as_u64().unwrap() >= 1);
    assert!(section_body(&mut repo2, MAIN_REF, cust_sec).contains("tier"));
    assert!(section_body(&mut repo2, MAIN_REF, ord_sec).contains("TIMESTAMP"));
    assert!(wisp_engine::query::fsck(&mut repo2).is_ok());
    assert!(wisp_engine::query::hot_check(&mut repo2, MAIN_REF).is_ok());
}

#[test]
fn merge_same_section_conflict_contract() {
    let cat = catalog();
    let meta = load_merge_scenario(
        cat.scenario("merge.same-section-conflict")
            .expect("merge.same-section-conflict"),
    )
    .expect("scenario.toml");
    assert_eq!(meta.expect_outcome, "merge_conflict");

    let (_dir, mut repo) = warehouse_repo();
    let branch_name = &meta.branch;
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
    assert_eq!(err.code, ExitCode::Conflict);
    let envelope = &err.details["conflicts"];
    assert_eq!(envelope["type"], "merge_conflict");
    let rows = envelope["conflicts"].as_array().expect("scope conflicts");
    assert!(!rows.is_empty());
    assert_eq!(rows[0]["type"], "scope_conflict");
    assert!(rows[0]["ours"].is_string());
    assert!(rows[0]["theirs"].is_string());

    let ours = rows[0]["ours"].as_str().unwrap();
    let theirs = rows[0]["theirs"].as_str().unwrap();
    let res_path = _dir.path().join("resolutions.json");
    std::fs::write(
        &res_path,
        format!(r#"{{"resolutions":[{{"ours":"{ours}","theirs":"{theirs}","take":"theirs"}}]}}"#,),
    )
    .unwrap();
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, Some(&res_path), false).unwrap();
    assert_eq!(merged["status"], "merged");
    assert!(section_body(&mut repo, MAIN_REF, ord_sec).contains("STRING"));
    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
}

#[test]
fn merge_nested_second_divergence_hot_check() {
    let (_dir, mut repo) = warehouse_repo();
    let branch_a = "task/nested-a";
    branch(&mut repo, branch_a, None).unwrap();
    let (cust_sec, cust_ver) = section_at_ref(&mut repo, branch_a, "tables/customers", "Schema");
    update_section_ref(
        &mut repo,
        branch_a,
        "process:branch-bot/1.0",
        "tables/customers",
        cust_sec,
        cust_ver,
        "nested tier column\n",
        "branch a",
        vec![],
    )
    .unwrap();
    merge_refs(&mut repo, branch_a, MAIN_REF, None, false).unwrap();
    let branch_b = "task/nested-b";
    branch(&mut repo, branch_b, None).unwrap();
    let (ord_sec, ord_ver) = section_at_ref(&mut repo, branch_b, "tables/orders", "Schema");
    update_section_ref(
        &mut repo,
        branch_b,
        "process:branch-bot/1.0",
        "tables/orders",
        ord_sec,
        ord_ver,
        "nested orders edit\n",
        "branch b",
        vec![],
    )
    .unwrap();
    let (_, ord_ver_m) = section_at(&mut repo, "tables/orders", "Schema");
    update_section(
        &mut repo,
        "process:main-bot/1.0",
        "tables/orders",
        ord_sec,
        ord_ver_m,
        "main after first merge\n",
        "main",
        vec![],
    )
    .unwrap();
    let err = merge_refs(&mut repo, branch_b, MAIN_REF, None, false).unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    let rows = err.details["conflicts"]["conflicts"].as_array().unwrap();
    let ours = rows[0]["ours"].as_str().unwrap();
    let theirs = rows[0]["theirs"].as_str().unwrap();
    let res_path = _dir.path().join("res2.json");
    std::fs::write(
        &res_path,
        format!(r#"{{"resolutions":[{{"ours":"{ours}","theirs":"{theirs}","take":"theirs"}}]}}"#),
    )
    .unwrap();
    merge_refs(&mut repo, branch_b, MAIN_REF, Some(&res_path), false).unwrap();
    assert!(section_body(&mut repo, MAIN_REF, cust_sec).contains("nested tier"));
    assert!(section_body(&mut repo, MAIN_REF, ord_sec).contains("nested orders"));
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

#[test]
fn merge_retract_survives_unrelated_branch_commit() {
    let (_dir, mut repo) = warehouse_repo();
    let branch_name = "task/retract-chain";
    branch(&mut repo, branch_name, None).unwrap();
    wisp_engine::erase::retract(
        &mut repo,
        branch_name,
        "tables/orders",
        wisp_model::Reason::Withdrawn,
        None,
        "human:alice",
    )
    .unwrap();
    let (cust_sec, cust_ver) = section_at(&mut repo, "tables/customers", "Schema");
    update_section(
        &mut repo,
        "process:main-bot/1.0",
        "tables/customers",
        cust_sec,
        cust_ver,
        "main customers diverge\n",
        "main diverge",
        vec![],
    )
    .unwrap();
    let merged = merge_refs(&mut repo, branch_name, MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "merged");
    let hot = repo.hot(MAIN_REF).unwrap();
    let retracted_paths: Vec<String> = Hot::retracted_all(hot.conn())
        .unwrap()
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert!(retracted_paths.iter().any(|p| p == "tables/orders"));
    assert!(section_body(&mut repo, MAIN_REF, cust_sec).contains("main customers"));
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

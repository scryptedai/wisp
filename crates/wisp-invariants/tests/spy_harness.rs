//! Shared helpers for SPY options playbook tests.

#![allow(dead_code)]

use wisp_corpus::BundleSpec;
use wisp_engine::headless::{apply, ApplyBatch, ApplyInput, ApplyOp};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_invariants::init_and_record_bundle;
use wisp_model::{ChangeId, ChangesetId, SectionId};
use wisp_store::Hot;

pub fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

pub fn fresh_spy_repo(spec: &BundleSpec) -> (tempfile::TempDir, std::path::PathBuf, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    (dir, root, repo)
}

pub fn section_at(
    repo: &mut Repository,
    concept_path: &str,
    heading: &str,
) -> (SectionId, ChangeId) {
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), concept_path)
        .unwrap()
        .unwrap_or_else(|| panic!("no concept {concept_path}"));
    let s = Hot::sections_of(hot.conn(), c.concept)
        .unwrap()
        .into_iter()
        .find(|s| s.heading == heading)
        .unwrap_or_else(|| panic!("no section {heading} in {concept_path}"));
    (s.section, s.version)
}

pub fn head_changeset(repo: &mut Repository) -> ChangesetId {
    let hot = repo.hot(MAIN_REF).unwrap();
    Hot::meta(hot.conn())
        .unwrap()
        .expect("hot meta")
        .head
}

pub fn update_section(
    repo: &mut Repository,
    actor: &str,
    path: &str,
    section: SectionId,
    base: ChangeId,
    content: &str,
    message: &str,
    inputs: Vec<ApplyInput>,
) -> Result<serde_json::Value, wisp_model::WispError> {
    let op = ApplyOp {
        op: "update_section".into(),
        path: format!("{}.md", path),
        section: Some(section),
        base_version: Some(base.to_string()),
        heading: None,
        level: None,
        content: Some(content.to_string()),
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
    };
    apply(
        repo,
        ApplyBatch {
            r#ref: None,
            base: None,
            actor: Some(actor.into()),
            message: Some(message.into()),
            ops: vec![op],
            inputs,
            assert: vec![],
            assert_head: None,
        },
        None,
    )
}

pub fn rec(repo: &mut Repository, actor: &str, message: &str) -> wisp_engine::record::RecordResult {
    record(
        repo,
        &RecordOptions {
            actor: Some(actor.into()),
            message: Some(message.into()),
            ..Default::default()
        },
    )
    .unwrap()
}

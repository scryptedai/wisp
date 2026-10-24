//! Async materialize: hot updates on headless commit; disk catches up on barriers (docs/WORKTREE.md).

mod spy_harness;

use spy_harness::*;
use wisp_engine::headless::{apply, ApplyBatch, ApplyOp};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_store::Hot;

fn init_repo(materialize_enabled: bool) -> (tempfile::TempDir, std::path::PathBuf, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("doc.md"),
        "---\ntype: Doc\n---\n## Body\n\nalpha\n",
    )
    .unwrap();
    Repository::init(&root, "0.2", &unlock()).unwrap();
    let mut repo = Repository::open(&root, &unlock()).unwrap();
    repo.manifest.materialize.enabled = materialize_enabled;
    repo.manifest
        .save(&root.join(".wisp/manifest.toml"))
        .unwrap();
    drop(repo);
    let mut repo = Repository::open(&root, &unlock()).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:test".into()),
            message: Some("initial".into()),
            ..Default::default()
        },
    )
    .unwrap();
    (dir, root, repo)
}

fn disk_body(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("doc.md")).unwrap()
}

fn hot_body(repo: &mut Repository) -> String {
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), "doc").unwrap().unwrap();
    let s = Hot::sections_of(hot.conn(), c.concept)
        .unwrap()
        .into_iter()
        .find(|s| s.heading == "Body")
        .unwrap();
    s.body
}

fn headless_update_body(repo: &mut Repository, content: &str) {
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), "doc").unwrap().unwrap();
    let s = Hot::sections_of(hot.conn(), c.concept)
        .unwrap()
        .into_iter()
        .find(|s| s.heading == "Body")
        .unwrap();
    let op = ApplyOp {
        op: "update_section".into(),
        path: "doc.md".into(),
        section: Some(s.section),
        base_version: Some(s.version.to_string()),
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
        ..Default::default()
    };
    apply(
        repo,
        ApplyBatch {
            r#ref: None,
            base: None,
            actor: Some("process:test".into()),
            message: Some("headless".into()),
            ops: vec![op],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        },
        None,
    )
    .unwrap();
}

#[test]
fn headless_updates_hot_before_disk_when_materialize_disabled() {
    let (_dir, root, mut repo) = init_repo(false);
    assert!(disk_body(&root).contains("alpha"));
    headless_update_body(&mut repo, "beta\n");
    assert_eq!(hot_body(&mut repo).trim(), "beta");
    assert!(
        disk_body(&root).contains("alpha"),
        "disk must lag hot when async publish is disabled"
    );
    repo.manifest.materialize.enabled = true;
    repo.schedule_materialize_full(MAIN_REF).unwrap();
    repo.flush_materialize().unwrap();
    assert!(disk_body(&root).contains("beta"));
    assert!(!disk_body(&root).contains("alpha\n"));
}

#[test]
fn headless_enqueue_then_flush_materialize_syncs_disk() {
    let (_dir, root, mut repo) = init_repo(true);
    headless_update_body(&mut repo, "gamma\n");
    assert_eq!(hot_body(&mut repo).trim(), "gamma");
    let st = repo.materialize_status();
    assert!(
        st.pending_jobs > 0 || st.pending_paths > 0 || !disk_body(&root).contains("gamma"),
        "headless commit must queue publish work or leave disk stale before barrier"
    );
    repo.flush_materialize().unwrap();
    assert_eq!(repo.materialize_status().pending_jobs, 0);
    assert!(disk_body(&root).contains("gamma"));
}

#[test]
fn record_commit_drains_materialize_backlog() {
    let (_dir, root, mut repo) = init_repo(true);
    headless_update_body(&mut repo, "delta\n");
    repo.flush_materialize().unwrap();
    assert!(disk_body(&root).contains("delta"));
    std::fs::write(
        root.join("notes-extra.md"),
        "---\ntype: Note\n---\n## N\n\nfrom disk\n",
    )
    .unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:test".into()),
            message: Some("pick up disk".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repo.materialize_status().pending_jobs, 0);
    let hot = repo.hot(MAIN_REF).unwrap();
    assert!(Hot::concept_by_path(hot.conn(), "notes-extra")
        .unwrap()
        .is_some());
}

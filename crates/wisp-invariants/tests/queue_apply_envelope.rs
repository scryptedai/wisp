//! Queue `apply` must return the same errors as direct `apply` (socket client parses `ok:false`).

#[cfg(unix)]
mod unix {
    use serde_json::json;
    use std::sync::Arc;
    use wisp_engine::commit_queue::CommitQueue;
    use wisp_engine::headless::{apply, apply_direct, ApplyBatch, ApplyOp};
    use wisp_engine::repo::{Repository, MAIN_REF};
    use wisp_model::{ChangeId, ExitCode, SectionId, WispError};
    use wisp_store::Hot;

    fn unlock() -> wisp_engine::repo::Unlock {
        std::env::remove_var("WISP_MASTER_KEY");
        wisp_engine::repo::Unlock::default()
    }

    fn seed_blue_node(repo: &mut Repository) -> (SectionId, ChangeId) {
        let batch = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-a/0.1".into()),
            message: Some("seed".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: "blue/node.md".into(),
                r#type: Some("concept".into()),
                content: Some("# blue\n".into()),
                frontmatter: Some(json!({ "title": "blue" })),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
        };
        apply_direct(repo, batch, None).expect("seed");
        let hot = repo.hot(MAIN_REF).unwrap();
        let c = Hot::concept_by_path(hot.conn(), "blue/node")
            .unwrap()
            .unwrap();
        let sec = Hot::sections_of(hot.conn(), c.concept)
            .unwrap()
            .into_iter()
            .next()
            .expect("section");
        (sec.section, sec.version)
    }

    fn enable_queue_manifest(repo: &mut Repository) {
        repo.manifest.queue.enabled = true;
        repo.manifest.queue.timeout_ms = 120_000;
        repo.manifest
            .save(&repo.wisp_dir.join("manifest.toml"))
            .unwrap();
    }

    struct QueueFixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        queue: Arc<CommitQueue>,
    }

    fn queue_fixture() -> QueueFixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let mut repo = Repository::init(&root, "0.2", &unlock()).unwrap();
        seed_blue_node(&mut repo);
        enable_queue_manifest(&mut repo);
        let cfg = repo.manifest.queue.clone();
        let wisp_dir = repo.wisp_dir.clone();
        drop(repo);
        let queue = CommitQueue::bind_unix(&wisp_dir, unlock(), cfg).expect("bind queue");
        QueueFixture {
            _dir: dir,
            root,
            queue,
        }
    }

    fn open_client(root: &std::path::Path) -> Repository {
        Repository::open(root, &unlock()).expect("open client")
    }

    fn dup_create_batch() -> ApplyBatch {
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-concepts/0.1".into()),
            message: Some("dup".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: "blue/node.md".into(),
                r#type: Some("concept".into()),
                content: Some("# blue again\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
        }
    }

    fn assert_path_conflict(err: &WispError) {
        assert_eq!(err.code, ExitCode::Conflict);
        assert_eq!(
            err.details["conflicts"][0]["type"].as_str(),
            Some("path_conflict")
        );
    }

    #[test]
    fn queue_apply_duplicate_path_matches_direct() {
        let fx = queue_fixture();
        let mut direct = open_client(&fx.root);
        let batch = dup_create_batch();
        let direct_err = apply_direct(&mut direct, batch.clone(), None).unwrap_err();
        assert_path_conflict(&direct_err);

        let mut queued = open_client(&fx.root);
        let queue_err = apply(&mut queued, batch, None).unwrap_err();
        assert_path_conflict(&queue_err);
    }

    #[test]
    fn queue_apply_section_conflict_matches_direct() {
        let fx = queue_fixture();
        let mut seed_repo = open_client(&fx.root);
        let (section, version) = {
            let hot = seed_repo.hot(MAIN_REF).unwrap();
            let c = Hot::concept_by_path(hot.conn(), "blue/node")
                .unwrap()
                .unwrap();
            let sec = Hot::sections_of(hot.conn(), c.concept)
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            (sec.section, sec.version)
        };

        let first = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-explorer-2/0.1".into()),
            message: Some("first update".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(section),
                base_version: Some(version.to_string()),
                content: Some("# blue\n\nUpdated.\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        };
        apply(&mut seed_repo, first, None).expect("first writer");

        let stale = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-explorer-1/0.1".into()),
            message: Some("stale update".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(section),
                base_version: Some(version.to_string()),
                content: Some("# blue\n\nStale.\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        };

        let mut direct = open_client(&fx.root);
        let direct_err = apply_direct(&mut direct, stale.clone(), None).unwrap_err();
        assert_eq!(direct_err.code, ExitCode::Conflict);
        assert_eq!(
            direct_err.details["conflicts"][0]["type"].as_str(),
            Some("section_conflict")
        );
        assert!(direct_err.details["conflicts"][0]["theirs"]["content"]
            .as_str()
            .unwrap()
            .contains("Updated."));

        let mut queued = open_client(&fx.root);
        let queue_err = apply(&mut queued, stale, None).unwrap_err();
        assert_eq!(queue_err.code, ExitCode::Conflict);
        assert_eq!(
            queue_err.details["conflicts"][0]["type"].as_str(),
            Some("section_conflict")
        );
    }

    #[test]
    fn queue_apply_not_found_matches_direct() {
        let fx = queue_fixture();
        let mut repo = open_client(&fx.root);
        let (section, version) = {
            let hot = repo.hot(MAIN_REF).unwrap();
            let c = Hot::concept_by_path(hot.conn(), "blue/node")
                .unwrap()
                .unwrap();
            let sec = Hot::sections_of(hot.conn(), c.concept)
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            (sec.section, sec.version)
        };

        let updated = apply(
            &mut repo,
            ApplyBatch {
                r#ref: Some(MAIN_REF.into()),
                base: None,
                actor: Some("loq-explorer-2/0.1".into()),
                message: Some("first update".into()),
                ops: vec![ApplyOp {
                    op: "update_section".into(),
                    path: "blue/node.md".into(),
                    section: Some(section),
                    base_version: Some(version.to_string()),
                    content: Some("# blue\n\nUpdated.\n".into()),
                    ..Default::default()
                }],
                inputs: vec![],
                assert: vec![],
                assert_head: None,
                inputs_complete: false,
            },
            None,
        )
        .unwrap();
        let new_version = updated["changes"][0]["version"]
            .as_str()
            .unwrap()
            .parse::<ChangeId>()
            .unwrap();

        apply(
            &mut repo,
            ApplyBatch {
                r#ref: Some(MAIN_REF.into()),
                base: None,
                actor: Some("loq-explorer-2/0.1".into()),
                message: Some("remove".into()),
                ops: vec![ApplyOp {
                    op: "remove_section".into(),
                    path: "blue/node.md".into(),
                    section: Some(section),
                    base_version: Some(new_version.to_string()),
                    ..Default::default()
                }],
                inputs: vec![],
                assert: vec![],
                assert_head: None,
                inputs_complete: false,
            },
            None,
        )
        .unwrap();

        let gone = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-explorer-1/0.1".into()),
            message: Some("update removed".into()),
            ops: vec![ApplyOp {
                op: "update_section".into(),
                path: "blue/node.md".into(),
                section: Some(section),
                base_version: Some(version.to_string()),
                content: Some("# gone\n".into()),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        };

        let mut direct = open_client(&fx.root);
        let direct_err = apply_direct(&mut direct, gone.clone(), None).unwrap_err();
        assert_eq!(direct_err.code, ExitCode::NotFound);

        let mut queued = open_client(&fx.root);
        let queue_err = apply(&mut queued, gone, None).unwrap_err();
        assert_eq!(queue_err.code, ExitCode::NotFound);
    }

    #[test]
    fn queue_apply_success_still_has_head() {
        let fx = queue_fixture();
        let mut repo = open_client(&fx.root);
        let batch = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("loq-a/0.1".into()),
            message: Some("create nature".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: "blue/nature.md".into(),
                r#type: Some("concept".into()),
                content: Some("# nature\n".into()),
                frontmatter: Some(json!({ "title": "nature" })),
                ..Default::default()
            }],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: true,
        };
        let out = apply(&mut repo, batch, None).expect("queued create");
        assert!(out.get("head").and_then(|x| x.as_str()).is_some());
        assert!(out.get("changeset").and_then(|x| x.as_str()).is_some());
        fx.queue.shutdown();
    }
}

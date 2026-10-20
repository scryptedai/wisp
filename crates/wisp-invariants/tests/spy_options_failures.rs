//! SPY options desk — expected failures (exit codes, conflicts, validation).

mod spy_harness;

use spy_harness::*;
use wisp_engine::headless::{apply, ApplyAssert, ApplyBatch, ApplyOp};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::MAIN_REF;
use wisp_invariants::catalog;
use wisp_model::{ChangeId, ExitCode, Reason, SectionId};
use wisp_store::Hot;

#[test]
fn spy_fail_record_hand_edited_index_md() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);
    std::fs::write(root.join("index.md"), "# desk tamper\n").unwrap();
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            message: Some("oops".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Refused);
}

#[test]
fn spy_fail_record_hand_edited_log_md() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);
    let log = root.join("strategy/log.md");
    assert!(log.is_file());
    std::fs::write(&log, "## 2099-01-01\n\nFake log entry\n").unwrap();
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Refused);
}

#[test]
fn spy_fail_nonconformant_scratch_file_validation() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);
    std::fs::create_dir_all(root.join("scratch")).unwrap();
    std::fs::write(root.join("scratch/no-yaml.md"), "Ticker tape only.\n").unwrap();
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Validation);
    std::fs::remove_file(root.join("scratch/no-yaml.md")).unwrap();
    rec(&mut repo, "human:tim", "remove scratch");
}

#[test]
fn spy_fail_section_conflict_unresolved_second_stale_base() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let strategy = "strategy/spy-calls-overview";
    let (exit_sec, exit_v0) = section_at(&mut repo, strategy, "Exit rules");
    update_section(
        &mut repo,
        "process:execution-bot/1.0",
        strategy,
        exit_sec,
        exit_v0,
        "First agent exit policy.\n",
        "bot A",
        vec![],
    )
    .unwrap();
    let (_, exit_v1) = section_at(&mut repo, strategy, "Exit rules");
    let err1 = update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        exit_sec,
        exit_v0,
        "Conflicting scanner exit.\n",
        "bot B stale",
        vec![],
    )
    .unwrap_err();
    assert_eq!(err1.code, ExitCode::Conflict);
    let err2 = update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        exit_sec,
        exit_v0,
        "Still wrong base.\n",
        "bot B still stale",
        vec![],
    )
    .unwrap_err();
    assert_eq!(err2.code, ExitCode::Conflict);
    let theirs: ChangeId = err1.details["conflicts"][0]["theirs"]["version"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(theirs, exit_v0);
    update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        exit_sec,
        theirs,
        "Merged after accepting theirs.\n",
        "resolved",
        vec![],
    )
    .unwrap();
    let (_, exit_v2) = section_at(&mut repo, strategy, "Exit rules");
    assert_ne!(exit_v1, exit_v2);
}

#[test]
fn spy_fail_assert_observation_version_mismatch() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let (entry_sec, entry_ver) = section_at(&mut repo, "strategy/spy-calls-overview", "Entry rules");
    let bogus = ChangeId::new();
    let batch = ApplyBatch {
        r#ref: None,
        base: None,
        actor: Some("process:news-scanner/1.0".into()),
        message: Some("trade on stale observation".into()),
        ops: vec![ApplyOp {
            op: "update_section".into(),
            path: "strategy/spy-calls-overview.md".into(),
            section: Some(entry_sec),
            base_version: Some(entry_ver.to_string()),
            heading: None,
            level: None,
            content: Some("Should not commit.\n".into()),
            after: None,
            key: None,
            value: None,
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        }],
        inputs: vec![],
        assert: vec![ApplyAssert {
            path: "observations/2026-10-18-cpi-print.md".into(),
            section: None,
            version: bogus,
        }],
        assert_head: None,
    };
    let err = apply(&mut repo, batch, None).unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    assert_eq!(err.details["conflicts"][0]["type"], "assertion_failed");
}

#[test]
fn spy_fail_assert_head_after_concurrent_commit() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let head_before = head_changeset(&mut repo);
    let (entry_sec, entry_ver) = section_at(&mut repo, "strategy/spy-calls-overview", "Entry rules");
    update_section(
        &mut repo,
        "process:risk-bot/0.2",
        "strategy/spy-calls-overview",
        entry_sec,
        entry_ver,
        "Risk bot moved entry rules first.\n",
        "risk preempt",
        vec![],
    )
    .unwrap();
    let batch = ApplyBatch {
        r#ref: None,
        base: None,
        actor: Some("process:news-scanner/1.0".into()),
        message: Some("expected quiet head".into()),
        ops: vec![ApplyOp {
            op: "verify".into(),
            path: "strategy/spy-calls-overview.md".into(),
            section: None,
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
        }],
        inputs: vec![],
        assert: vec![],
        assert_head: Some(head_before),
    };
    let err = apply(&mut repo, batch, None).unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    assert_eq!(err.details["conflicts"][0]["type"], "assertion_failed");
}

#[test]
fn spy_fail_duplicate_wisp_id_incremental_record_treats_copy_as_move() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);
    let src = std::fs::read_to_string(root.join("observations/2026-10-18-cpi-print.md")).unwrap();
    let parsed = wisp_parse::parse_file(&src);
    let original_id = parsed.wisp_id("wisp_id").expect("cpi has wisp_id after record");
    std::fs::write(root.join("observations/cpi-print-copy.md"), &src).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            message: Some("agent duplicated CPI note; only copy is dirty".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let moved = Hot::concept_by_path(hot.conn(), "observations/cpi-print-copy")
        .unwrap()
        .expect("concept at copy path");
    assert_eq!(
        moved.concept,
        original_id,
        "incremental record sees one dirty file → §7.4.1 move, not §7.4.2 copy"
    );
    assert!(
        Hot::concept_by_path(hot.conn(), "observations/2026-10-18-cpi-print")
            .unwrap()
            .is_none(),
        "canonical path should no longer own the concept"
    );
}

#[test]
fn spy_fail_duplicate_wisp_id_full_record_remints_copy() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);
    let src = std::fs::read_to_string(root.join("observations/2026-10-18-cpi-print.md")).unwrap();
    let parsed = wisp_parse::parse_file(&src);
    let original_id = parsed.wisp_id("wisp_id").expect("cpi has wisp_id after record");
    std::fs::write(root.join("observations/cpi-print-copy.md"), &src).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            message: Some("full record: both CPI files present".into()),
            full: true,
            ..Default::default()
        },
    )
    .unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let orig = Hot::concept_by_path(hot.conn(), "observations/2026-10-18-cpi-print")
        .unwrap()
        .expect("original path");
    let copy = Hot::concept_by_path(hot.conn(), "observations/cpi-print-copy")
        .unwrap()
        .expect("copy path");
    assert_eq!(orig.concept, original_id);
    assert_ne!(copy.concept, original_id, "§7.4.2 remint when both files recorded");
}

#[test]
fn spy_fail_retract_twice() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    wisp_engine::erase::retract(
        &mut repo,
        MAIN_REF,
        "observations/2026-10-17-premarket",
        Reason::Superseded,
        None,
        "human:tim",
    )
    .unwrap();
    let err = wisp_engine::erase::retract(
        &mut repo,
        MAIN_REF,
        "observations/2026-10-17-premarket",
        Reason::Superseded,
        None,
        "human:tim",
    )
    .unwrap_err();
    assert!(
        err.code == ExitCode::Conflict || err.code == ExitCode::NotFound,
        "{:?}",
        err
    );
}

#[test]
fn spy_fail_headless_update_on_retracted_observation() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let path = "observations/2026-10-17-premarket";
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), path).unwrap().expect("obs");
    let sec = Hot::sections_of(hot.conn(), c.concept)
        .unwrap()
        .into_iter()
        .find(|s| s.level > 0)
        .map(|s| (s.section, s.version))
        .or_else(|| {
            Hot::sections_of(hot.conn(), c.concept)
                .unwrap()
                .first()
                .map(|s| (s.section, s.version))
        })
        .expect("section");
    wisp_engine::erase::retract(
        &mut repo,
        MAIN_REF,
        path,
        Reason::Superseded,
        None,
        "human:tim",
    )
    .unwrap();
    let err = update_section(
        &mut repo,
        "process:news-scanner/1.0",
        path,
        sec.0,
        sec.1,
        "stale body\n",
        "should fail",
        vec![],
    )
    .unwrap_err();
    assert!(
        err.code == ExitCode::Conflict || err.code == ExitCode::NotFound,
        "{:?}",
        err
    );
}

#[test]
fn spy_fail_headless_unknown_concept_path() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let err = update_section(
        &mut repo,
        "process:news-scanner/1.0",
        "observations/no-such-print",
        SectionId::new(),
        ChangeId::new(),
        "x\n",
        "bad path",
        vec![],
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::NotFound);
}

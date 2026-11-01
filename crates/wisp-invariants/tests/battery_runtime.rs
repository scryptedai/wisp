//! Runtime acceptance battery (`contrib/battery-runtime/cases.toml`).

mod spy_harness;

use spy_harness::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use wisp_engine::headless::{ApplyBatch, ApplyOp};
use wisp_engine::manifest::RuntimeSection;
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_engine::runtime::{self, RuntimeServer};
use wisp_invariants::catalog;
use wisp_model::ExitCode;

const CASES_TOML: &str = include_str!("../../../contrib/battery-runtime/cases.toml");

fn spy_pack_paths() -> Vec<String> {
    vec![
        "strategy/spy-calls-overview".into(),
        "signals/news-signal-rubric".into(),
        "journal/trade-log".into(),
    ]
}

fn pack_contains_text(pack: &serde_json::Value, needle: &str) -> bool {
    pack["concepts"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| c["sections"].as_array())
        .flatten()
        .any(|s| s["text"].as_str().is_some_and(|t| t.contains(needle)))
}

fn enable_runtime(root: &std::path::Path) {
    let path = root.join(".wisp/manifest.toml");
    let mut m = wisp_engine::manifest::Manifest::load(&path).expect("manifest");
    m.runtime.enabled = true;
    m.save(&path).expect("save manifest");
}

fn runtime_cfg() -> RuntimeSection {
    RuntimeSection {
        enabled: true,
        timeout_ms: 120_000,
        ..Default::default()
    }
}

#[test]
fn battery_runtime_misconfigured_no_socket() {
    let _env = runtime::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    drop(repo);
    enable_runtime(root);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let err = match runtime::open_repo_or_misconfigured(root, &unlock()) {
        Ok(_) => panic!("expected direct open forbidden"),
        Err(e) => e,
    };
    assert_eq!(err.code, ExitCode::Usage);
    let err = runtime::rpc_cat(
        &root.join(".wisp"),
        "human:test",
        MAIN_REF,
        "note.md",
        None,
        5_000,
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Usage);
    let _cases = CASES_TOML;
}

#[test]
fn battery_runtime_apply_success_has_head() {
    let _env = runtime::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    enable_runtime(root);
    drop(repo);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let _rt = RuntimeServer::spawn(root.to_path_buf(), unlock(), runtime_cfg()).expect("spawn");
    let batch = ApplyBatch {
        r#ref: Some(MAIN_REF.into()),
        actor: Some("battery/runtime".into()),
        message: Some("runtime apply ok".into()),
        ops: vec![ApplyOp {
            op: "create_concept".into(),
            path: "notes/runtime-apply.md".into(),
            r#type: Some("note".into()),
            content: Some("# Runtime apply\n\nok.\n".into()),
            ..Default::default()
        }],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
        inputs_complete: false,
        base: None,
    };
    let out = runtime::rpc_apply(
        &root.join(".wisp"),
        "battery/runtime",
        serde_json::to_value(&batch).unwrap(),
        Some(MAIN_REF),
        120_000,
    )
    .expect("apply rpc");
    assert!(out.get("head").and_then(|v| v.as_str()).is_some());
}

#[test]
fn battery_runtime_feed_after_apply() {
    let _env = runtime::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    enable_runtime(root);
    drop(repo);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let _rt = RuntimeServer::spawn(root.to_path_buf(), unlock(), runtime_cfg()).expect("spawn");
    let wisp_dir = root.join(".wisp");
    let feed0 = runtime::rpc_feed(&wisp_dir, "battery/runtime", 0, 5_000).expect("feed");
    let head0 = feed0["seq"].as_i64().unwrap_or(0);
    let batch = ApplyBatch {
        r#ref: Some(MAIN_REF.into()),
        actor: Some("battery/runtime".into()),
        message: Some("feed test".into()),
        ops: vec![ApplyOp {
            op: "create_concept".into(),
            path: "notes/feed.md".into(),
            r#type: Some("note".into()),
            content: Some("# feed\n".into()),
            ..Default::default()
        }],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
        inputs_complete: false,
        base: None,
    };
    runtime::rpc_apply(
        &wisp_dir,
        "battery/runtime",
        serde_json::to_value(&batch).unwrap(),
        Some(MAIN_REF),
        120_000,
    )
    .expect("apply");
    let feed1 = runtime::rpc_feed(&wisp_dir, "battery/runtime", head0, 5_000).expect("feed after");
    let events = feed1["events"].as_array().expect("events");
    assert!(
        !events.is_empty(),
        "feed should list ledger events after apply"
    );
}

#[test]
fn battery_runtime_apply_failure_no_head() {
    let _env = runtime::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    enable_runtime(root);
    drop(repo);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let _rt = RuntimeServer::spawn(root.to_path_buf(), unlock(), runtime_cfg()).expect("spawn");
    let batch = ApplyBatch {
        r#ref: Some(MAIN_REF.into()),
        actor: Some("battery/runtime".into()),
        message: Some("dup".into()),
        ops: vec![
            ApplyOp {
                op: "create_concept".into(),
                path: "notes/dup.md".into(),
                r#type: Some("note".into()),
                content: Some("one\n".into()),
                ..Default::default()
            },
            ApplyOp {
                op: "create_concept".into(),
                path: "notes/dup.md".into(),
                r#type: Some("note".into()),
                content: Some("two\n".into()),
                ..Default::default()
            },
        ],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
        inputs_complete: false,
        base: None,
    };
    let err = runtime::rpc_apply(
        &root.join(".wisp"),
        "battery/runtime",
        serde_json::to_value(&batch).unwrap(),
        Some(MAIN_REF),
        120_000,
    )
    .unwrap_err();
    assert_ne!(err.code, ExitCode::Success);
}

#[test]
#[ignore = "heavy; run with WISP_RUNTIME_BATTERY=1"]
fn battery_runtime_idle_cat() {
    if std::env::var("WISP_RUNTIME_BATTERY").is_err() {
        return;
    }
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("bundle");
    let (_dir, root, repo) = fresh_spy_repo(spec);
    enable_runtime(&root);
    drop(repo);
    let _rt = RuntimeServer::spawn(root.clone(), unlock(), runtime_cfg()).expect("runtime spawn");
    let wisp_dir = root.join(".wisp");
    let path = "strategy/spy-calls-overview";
    // Warm cache
    runtime::rpc_cat(&wisp_dir, "battery/runtime", MAIN_REF, path, None, 5_000).expect("warm cat");
    let n = 1_000usize;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        runtime::rpc_cat(&wisp_dir, "battery/runtime", MAIN_REF, path, None, 5_000)
            .expect("cat rpc");
        samples.push(t0.elapsed());
    }
    samples.sort();
    let p99 = samples[(n * 99) / 100];
    eprintln!(
        "idle_cat p99_ms={:.3} (cases.toml gate: 2ms)",
        p99.as_secs_f64() * 1000.0
    );
    assert!(
        p99 < Duration::from_millis(if cfg!(debug_assertions) { 25 } else { 2 }),
        "p99 cat latency {p99:?} exceeds gate"
    );
}

#[test]
#[ignore = "heavy; run with WISP_RUNTIME_BATTERY=1"]
fn battery_runtime_warm_pack() {
    if std::env::var("WISP_RUNTIME_BATTERY").is_err() {
        return;
    }
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("bundle");
    let (_dir, root, repo) = fresh_spy_repo(spec);
    enable_runtime(&root);
    drop(repo);
    let _rt = RuntimeServer::spawn(root.clone(), unlock(), runtime_cfg()).expect("spawn");
    let wisp_dir = root.join(".wisp");
    let paths = spy_pack_paths();
    let budget = Some(24_000usize);
    let _ = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        None,
        true,
        120_000,
    )
    .expect("warm pack");
    let seq = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        None,
        true,
        120_000,
    )
    .expect("pack")["seq"]
        .as_u64()
        .expect("seq");
    let mut handles = Vec::new();
    for i in 0..32 {
        let wisp_dir = wisp_dir.clone();
        let paths = paths.clone();
        handles.push(thread::spawn(move || {
            let v = runtime::rpc_pack(
                &wisp_dir,
                &format!("client-{i}"),
                MAIN_REF,
                &paths,
                budget,
                Some(seq),
                false,
                120_000,
            )
            .expect("pack hit");
            assert_eq!(v["seq"].as_u64(), Some(seq));
            assert_eq!(v["fresh"].as_bool(), Some(false));
        }));
    }
    for h in handles {
        h.join().expect("join");
    }
}

#[test]
#[ignore = "heavy; run with WISP_RUNTIME_BATTERY=1"]
fn battery_runtime_append_delta_pack() {
    if std::env::var("WISP_RUNTIME_BATTERY").is_err() {
        return;
    }
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("bundle");
    let (_dir, root, repo) = fresh_spy_repo(spec);
    enable_runtime(&root);
    drop(repo);
    let _rt = RuntimeServer::spawn(root.clone(), unlock(), runtime_cfg()).expect("spawn");
    let wisp_dir = root.join(".wisp");
    let paths = spy_pack_paths();
    let budget = Some(24_000usize);
    let concept_path = "strategy/spy-calls-overview";
    let cat_out = runtime::rpc_cat(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        concept_path,
        None,
        5_000,
    )
    .expect("cat for section id");
    let section = cat_out["sections"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|s| s["section"].as_str())
        .expect("section id");
    let before = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        None,
        true,
        120_000,
    )
    .expect("pack before append");
    let seq_before = before["seq"].as_u64().expect("seq");
    assert_eq!(before["fresh"].as_bool(), Some(true));
    let marker = "runtime-append-delta-marker\n";
    let batch = ApplyBatch {
        r#ref: Some(MAIN_REF.into()),
        actor: Some("battery/runtime".into()),
        message: Some("append delta pack".into()),
        ops: vec![ApplyOp {
            op: "append_section".into(),
            path: format!("{concept_path}.md"),
            section: section.parse().ok(),
            content: Some(marker.into()),
            ..Default::default()
        }],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
        inputs_complete: false,
        base: None,
    };
    runtime::rpc_apply(
        &wisp_dir,
        "battery/runtime",
        serde_json::to_value(&batch).unwrap(),
        Some(MAIN_REF),
        120_000,
    )
    .expect("append apply");
    let after = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        None,
        true,
        120_000,
    )
    .expect("pack after append");
    let seq_after = after["seq"].as_u64().expect("seq");
    assert!(seq_after > seq_before, "seq should bump after append");
    assert!(
        pack_contains_text(&after, marker.trim_end()),
        "pack should include appended text"
    );
    let stitched_hit = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        Some(seq_after),
        false,
        120_000,
    )
    .expect("pack cache hit after stitch");
    assert_eq!(stitched_hit["fresh"].as_bool(), Some(false));
    let delta_only = runtime::rpc_pack(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        &paths,
        budget,
        Some(seq_before),
        false,
        120_000,
    )
    .expect("pack delta");
    assert_eq!(delta_only["fresh"].as_bool(), Some(true));
    assert!(delta_only.get("delta").and_then(|d| d.as_array()).is_some());
    let _cases = CASES_TOML;
}

#[test]
#[ignore = "heavy; run with WISP_RUNTIME_BATTERY=1"]
fn battery_runtime_read_during_write() {
    if std::env::var("WISP_RUNTIME_BATTERY").is_err() {
        return;
    }
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("bundle");
    let (_dir, root, repo) = fresh_spy_repo(spec);
    drop(repo);
    enable_runtime(&root);
    let _rt = RuntimeServer::spawn(root.clone(), unlock(), runtime_cfg()).expect("spawn");
    let wisp_dir = root.join(".wisp");
    runtime::rpc_cat(
        &wisp_dir,
        "battery/runtime",
        MAIN_REF,
        "strategy/spy-calls-overview",
        None,
        5_000,
    )
    .expect("warm");
    let exit7 = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut handles = Vec::new();
    for i in 0..32 {
        let wisp_dir = wisp_dir.clone();
        let exit7 = Arc::clone(&exit7);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = runtime::rpc_cat(
                    &wisp_dir,
                    &format!("reader-{i}"),
                    MAIN_REF,
                    "strategy/spy-calls-overview",
                    None,
                    5_000,
                ) {
                    if e.code == ExitCode::WriteTimeout {
                        exit7.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    let wisp_w = wisp_dir.clone();
    let writer = thread::spawn(move || {
        for k in 0..20 {
            let batch = ApplyBatch {
                r#ref: Some(MAIN_REF.into()),
                actor: Some("writer/runtime".into()),
                message: Some("read-during-write".into()),
                ops: vec![ApplyOp {
                    op: "create_concept".into(),
                    path: format!("notes/w{k}.md"),
                    r#type: Some("note".into()),
                    content: Some(format!("# w{k}\n\nbody\n")),
                    ..Default::default()
                }],
                inputs: vec![],
                assert: vec![],
                assert_head: None,
                inputs_complete: false,
                base: None,
            };
            let _ = runtime::rpc_apply(
                &wisp_w,
                "writer/runtime",
                serde_json::to_value(&batch).unwrap(),
                Some(MAIN_REF),
                120_000,
            );
        }
    });
    writer.join().expect("writer");
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    assert_eq!(
        exit7.load(Ordering::Relaxed),
        0,
        "expected zero exit-7 style busy errors during concurrent cats"
    );
}

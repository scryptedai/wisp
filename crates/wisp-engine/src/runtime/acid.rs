//! Runtime load acid tests (fleet-shaped contention).

use crate::headless::{ApplyBatch, ApplyOp};
use crate::manifest::RuntimeSection;
use crate::repo::{Repository, Unlock, MAIN_REF};
use crate::runtime::{self, RuntimeServer};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use wisp_model::ExitCode;

fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

fn runtime_cfg() -> RuntimeSection {
    RuntimeSection {
        enabled: true,
        timeout_ms: 120_000,
        ..Default::default()
    }
}

/// Hotaru-shaped: many warmed cats on two log-like paths while a writer appends elsewhere.
#[test]
fn acid_hotaru_warmed_cats_under_append() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    let mut m = repo.manifest.clone();
    m.runtime.enabled = true;
    m.save(&repo.wisp_dir.join("manifest.toml")).unwrap();
    drop(repo);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let _rt = RuntimeServer::spawn(root.to_path_buf(), unlock(), runtime_cfg()).expect("spawn");
    let wisp_dir = root.join(".wisp");
    let hot_paths = ["notes/a.md", "notes/b.md"];
    for p in hot_paths {
        let batch = ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            actor: Some("acid/setup".into()),
            message: Some("seed".into()),
            ops: vec![ApplyOp {
                op: "create_concept".into(),
                path: p.into(),
                r#type: Some("note".into()),
                content: Some("# seed\n\nbody\n".into()),
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
            "acid/setup",
            serde_json::to_value(&batch).unwrap(),
            Some(MAIN_REF),
            30_000,
        )
        .expect("seed");
    }
    for p in hot_paths {
        let path = p.strip_suffix(".md").unwrap_or(p);
        runtime::rpc_cat(&wisp_dir, "acid/warm", MAIN_REF, path, None, 5_000).expect("warm cat");
    }
    let exit7 = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut handles = Vec::new();
    for i in 0..32 {
        let wisp_dir = wisp_dir.clone();
        let exit7 = Arc::clone(&exit7);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut samples = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                for path in hot_paths {
                    let path = path.strip_suffix(".md").unwrap_or(path);
                    let t0 = Instant::now();
                    match runtime::rpc_cat(
                        &wisp_dir,
                        &format!("reader-{i}"),
                        MAIN_REF,
                        path,
                        None,
                        5_000,
                    ) {
                        Ok(v) => {
                            samples.push(t0.elapsed());
                            assert_eq!(
                                v.get("cache").and_then(|c| c.as_str()),
                                Some("hit"),
                                "fleet cats should be cache hits after warm"
                            );
                        }
                        Err(e) if e.code == ExitCode::WriteTimeout => {
                            exit7.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => panic!("unexpected cat error: {e:?}"),
                    }
                }
            }
            samples
        }));
    }
    let wisp_w = wisp_dir.clone();
    let writer = thread::spawn(move || {
        for k in 0..40 {
            let batch = ApplyBatch {
                r#ref: Some(MAIN_REF.into()),
                actor: Some("acid/writer".into()),
                message: Some("write storm".into()),
                ops: vec![ApplyOp {
                    op: "create_concept".into(),
                    path: format!("notes/w{k}.md"),
                    r#type: Some("note".into()),
                    content: Some(format!("# w{k}\n\n")),
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
                "acid/writer",
                serde_json::to_value(&batch).unwrap(),
                Some(MAIN_REF),
                120_000,
            );
        }
    });
    thread::sleep(Duration::from_millis(500));
    writer.join().expect("writer");
    stop.store(true, Ordering::Relaxed);
    let mut all_samples = Vec::new();
    for h in handles {
        all_samples.extend(h.join().expect("join"));
    }
    all_samples.sort();
    let p99 = all_samples
        .get(all_samples.len().saturating_mul(99) / 100)
        .copied()
        .unwrap_or_default();
    eprintln!(
        "acid_hotaru warmed cat p99_ms={:.3} samples={}",
        p99.as_secs_f64() * 1000.0,
        all_samples.len()
    );
    assert_eq!(
        exit7.load(Ordering::Relaxed),
        0,
        "cache-hit cats must not exit 7 under append load"
    );
    let gate_ms = if cfg!(debug_assertions) { 50.0 } else { 5.0 };
    assert!(
        p99.as_secs_f64() * 1000.0 < gate_ms,
        "cache-hit cat p99 {p99:?} exceeds {gate_ms}ms gate"
    );
}

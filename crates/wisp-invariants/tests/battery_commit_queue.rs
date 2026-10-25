//! Commit queue battery: `contrib/battery-commit-queue/cases.toml`.

mod spy_harness;

use serde::Deserialize;
use spy_harness::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wisp_engine::commit_queue::CommitQueue;
use wisp_engine::headless::{apply, ApplyBatch, ApplyOp};
use wisp_engine::manifest::{CommitQueueSection, Manifest};
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::catalog;
use wisp_model::{ChangeId, ExitCode, SectionId};

const CASES_TOML: &str = include_str!("../../../contrib/battery-commit-queue/cases.toml");

#[derive(Debug, Deserialize)]
struct CasesFile {
    meta: Meta,
    phase: Vec<Phase>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    bundle_id: String,
    arena_path: String,
    arena_heading_prefix: String,
    default_writers: usize,
    ops_per_writer: usize,
    swarm_writers: usize,
    swarm_ops_per_writer: usize,
    busy_timeout_ms: u64,
    commit_retries: u32,
    queue_timeout_ms: u64,
    coalesce_window_us: u64,
}

#[derive(Debug, Deserialize)]
struct Phase {
    id: String,
    kind: String,
    transport: String,
    writers: Option<usize>,
    ops_per_writer: Option<usize>,
    busy_timeout_ms: Option<u64>,
    commit_retries: Option<u32>,
    expect_some_exit7: Option<bool>,
    expect_all_ok: Option<bool>,
    expect_fewer_commits_than_ops: Option<bool>,
    shared_actor: Option<String>,
    coalesce: Option<bool>,
    heavy: Option<bool>,
}

struct MetricRow {
    phase: String,
    step: String,
    wall_ms: f64,
    exit7: u32,
    queue_timeout: u32,
    successes: u32,
    failures: u32,
    writer_commits: u64,
    note: String,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn write_manifest_policy(
    root: &Path,
    busy_timeout_ms: u64,
    commit_retries: u32,
    queue_enabled: bool,
) {
    let path = root.join(".wisp/manifest.toml");
    let mut m = Manifest::load(&path).expect("manifest");
    m.commit.busy_timeout_ms = busy_timeout_ms;
    m.commit.retries = commit_retries;
    m.queue.enabled = queue_enabled;
    m.save(&path).expect("save manifest");
}

fn fresh_repo(meta: &Meta) -> (tempfile::TempDir, PathBuf) {
    let cat = catalog();
    let spec = cat.bundle(&meta.bundle_id).expect("bundle");
    let (dir, root, repo) = fresh_spy_repo(spec);
    write_manifest_policy(&root, meta.busy_timeout_ms, meta.commit_retries, true);
    drop(repo);
    (dir, root)
}

fn arena_heading(prefix: &str, i: usize) -> String {
    format!("{prefix}{i}")
}

fn update_op(path: &str, section: SectionId, base: ChangeId, content: &str) -> ApplyOp {
    ApplyOp {
        op: "update_section".into(),
        path: format!("{path}.md"),
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
        ..Default::default()
    }
}

fn run_disjoint_parallel(meta: &Meta, phase: &Phase, rows: &mut Vec<MetricRow>) {
    let writers = phase.writers.unwrap_or(if phase.heavy == Some(true) {
        meta.swarm_writers
    } else {
        meta.default_writers
    });
    let ops = phase
        .ops_per_writer
        .unwrap_or(if phase.heavy == Some(true) {
            meta.swarm_ops_per_writer
        } else {
            meta.ops_per_writer
        });
    let busy = phase.busy_timeout_ms.unwrap_or(meta.busy_timeout_ms);
    let retries = phase.commit_retries.unwrap_or(meta.commit_retries);
    let (_dir, root) = fresh_repo(meta);
    let queue_enabled = phase.transport == "queued";
    write_manifest_policy(&root, busy, retries, queue_enabled);

    let mut repo = Repository::open(&root, &unlock()).expect("open");
    let sections = (0..writers)
        .map(|i| {
            let h = arena_heading(&meta.arena_heading_prefix, i);
            section_at(&mut repo, &meta.arena_path, &h)
        })
        .collect::<Vec<_>>();
    drop(repo);

    let queued = phase.transport == "queued";
    let queue = if queued {
        let qcfg = CommitQueueSection {
            enabled: true,
            timeout_ms: meta.queue_timeout_ms,
            coalesce_window_us: meta.coalesce_window_us,
            coalesce: phase.coalesce.unwrap_or(false),
        };
        Some(CommitQueue::spawn(root.clone(), unlock(), qcfg).expect("queue"))
    } else {
        None
    };

    let total_exit7 = Arc::new(AtomicU32::new(0));
    let total_qt = Arc::new(AtomicU32::new(0));
    let total_ok = Arc::new(AtomicU32::new(0));
    let total_fail = Arc::new(AtomicU32::new(0));
    let path = meta.arena_path.clone();
    let shared_actor = phase.shared_actor.clone();
    let queue_timeout_ms = meta.queue_timeout_ms;

    let handles: Vec<_> = sections
        .into_iter()
        .enumerate()
        .map(|(i, (sec, mut base))| {
            let root = root.clone();
            let path = path.clone();
            let queue = queue.clone();
            let total_exit7 = Arc::clone(&total_exit7);
            let total_qt = Arc::clone(&total_qt);
            let total_ok = Arc::clone(&total_ok);
            let total_fail = Arc::clone(&total_fail);
            let shared_actor = shared_actor.clone();
            std::thread::spawn(move || {
                let actor = shared_actor
                    .clone()
                    .unwrap_or_else(|| format!("loq-explorer-{}/0.1", i + 1));
                let mut local_exit7 = 0u32;
                let mut local_qt = 0u32;
                let mut local_ok = 0u32;
                let mut local_fail = 0u32;
                let t0 = Instant::now();
                let mut direct_repo = if queue.is_none() {
                    Some(Repository::open(&root, &unlock()).unwrap())
                } else {
                    None
                };
                for k in 0..ops {
                    let content = format!("explorer {i} op {k} — disjoint section write.\n");
                    let op = update_op(&path, sec, base, &content);
                    let batch = ApplyBatch {
                        r#ref: Some(MAIN_REF.into()),
                        base: None,
                        actor: Some(actor.clone()),
                        message: Some("commit-queue-battery".into()),
                        ops: vec![op],
                        inputs: vec![],
                        assert: vec![],
                        assert_head: None,
                        inputs_complete: false,
                    };
                    let r = if let Some(q) = &queue {
                        q.submit(
                            batch,
                            Some(MAIN_REF),
                            Duration::from_millis(queue_timeout_ms),
                        )
                    } else {
                        apply(direct_repo.as_mut().unwrap(), batch, None)
                    };
                    match r {
                        Ok(v) => {
                            local_ok += 1;
                            base = v["changes"][0]["version"]
                                .as_str()
                                .unwrap()
                                .parse()
                                .unwrap();
                        }
                        Err(e) if e.code == ExitCode::WriteTimeout => {
                            local_exit7 += 1;
                            local_fail += 1;
                        }
                        Err(e) if e.code == ExitCode::QueueTimeout => {
                            local_qt += 1;
                            local_fail += 1;
                        }
                        Err(e) => panic!("writer {i} op {k}: {e:?}"),
                    }
                }
                total_exit7.fetch_add(local_exit7, Ordering::Relaxed);
                total_qt.fetch_add(local_qt, Ordering::Relaxed);
                total_ok.fetch_add(local_ok, Ordering::Relaxed);
                total_fail.fetch_add(local_fail, Ordering::Relaxed);
                (
                    i,
                    ms(t0.elapsed()),
                    local_exit7,
                    local_qt,
                    local_ok,
                    local_fail,
                )
            })
        })
        .collect();

    let joined: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let writer_commits = queue
        .as_ref()
        .map(|q| q.stats().writer_commits)
        .unwrap_or(0);
    if let Some(q) = &queue {
        q.shutdown();
    }
    for (i, wall, e7, qt, ok, fail) in joined {
        rows.push(MetricRow {
            phase: phase.id.clone(),
            step: format!("writer_{i}"),
            wall_ms: wall,
            exit7: e7,
            queue_timeout: qt,
            successes: ok,
            failures: fail,
            writer_commits,
            note: format!("{ok}/{ops} ops ok"),
        });
    }

    let exit7 = total_exit7.load(Ordering::Relaxed);
    let qt = total_qt.load(Ordering::Relaxed);
    let ok = total_ok.load(Ordering::Relaxed);
    let fail = total_fail.load(Ordering::Relaxed);
    let total_ops = (writers * ops) as u32;

    if phase.expect_some_exit7 == Some(true) {
        assert!(
            exit7 > 0 || fail > 0,
            "{}: expected direct-path pressure (exit7={exit7}, fail={fail})",
            phase.id
        );
    }
    if phase.expect_all_ok == Some(true) {
        assert_eq!(
            fail, 0,
            "{}: expected all ok (fail={fail}, exit7={exit7}, qt={qt})",
            phase.id
        );
        assert_eq!(ok, total_ops, "{}", phase.id);
    }
    if phase.expect_fewer_commits_than_ops == Some(true) {
        assert!(
            writer_commits > 0 && writer_commits < total_ops as u64,
            "{}: expected coalesce to reduce writer commits (commits={writer_commits}, ops={total_ops})",
            phase.id
        );
    }

    let mut repo = Repository::open(&root, &unlock()).unwrap();
    assert!(
        wisp_engine::query::fsck(&mut repo).unwrap()["ok"]
            .as_bool()
            .unwrap(),
        "fsck after {}",
        phase.id
    );
}

fn print_metrics(rows: &[MetricRow]) {
    eprintln!("\nbattery-commit-queue — metrics (ms)");
    eprintln!(
        "{:<26} {:<10} {:>8} {:>6} {:>6} {:>4} {:>4} {:>8} note",
        "phase", "step", "wall_ms", "exit7", "q_tmo", "ok", "fail", "commits"
    );
    for r in rows {
        eprintln!(
            "{:<26} {:<10} {:>8.3} {:>6} {:>6} {:>4} {:>4} {:>8} {}",
            r.phase,
            r.step,
            r.wall_ms,
            r.exit7,
            r.queue_timeout,
            r.successes,
            r.failures,
            r.writer_commits,
            r.note
        );
    }
    eprintln!();
}

fn run_phases(meta: &Meta, phases: &[&Phase], rows: &mut Vec<MetricRow>) {
    for phase in phases {
        if phase.kind != "disjoint_parallel" {
            panic!("unknown kind {}", phase.kind);
        }
        run_disjoint_parallel(meta, phase, rows);
    }
}

#[test]
fn battery_commit_queue() {
    let cases: CasesFile = toml::from_str(CASES_TOML).expect("cases");
    let mut rows = Vec::new();
    let phases = cases
        .phase
        .iter()
        .filter(|p| p.heavy != Some(true))
        .collect::<Vec<_>>();
    run_phases(&cases.meta, &phases, &mut rows);
    print_metrics(&rows);
}

#[test]
#[ignore = "heavy; run with WISP_COMMIT_QUEUE_BATTERY=1"]
fn battery_commit_queue_swarm() {
    if std::env::var("WISP_COMMIT_QUEUE_BATTERY").as_deref() != Ok("1") {
        return;
    }
    let cases: CasesFile = toml::from_str(CASES_TOML).expect("cases");
    let mut rows = Vec::new();
    let phases = cases
        .phase
        .iter()
        .filter(|p| p.heavy == Some(true))
        .collect::<Vec<_>>();
    run_phases(&cases.meta, &phases, &mut rows);
    print_metrics(&rows);
}

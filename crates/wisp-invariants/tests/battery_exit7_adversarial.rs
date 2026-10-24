//! Exit-7 write-queue battery: `contrib/battery-exit7-adversarial/cases.toml`.

mod spy_harness;

use serde::Deserialize;
use spy_harness::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wisp_engine::headless::{apply, ApplyBatch, ApplyOp};
use wisp_engine::manifest::Manifest;
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_engine::timing::{with_profile, PhaseSample};
use wisp_invariants::catalog;
use wisp_model::{ChangeId, ExitCode, SectionId, WispError};

const CASES_TOML: &str = include_str!("../../../contrib/battery-exit7-adversarial/cases.toml");

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
    heavy_writers: usize,
    busy_timeout_ms: u64,
    commit_retries: u32,
}

#[derive(Debug, Deserialize)]
struct Phase {
    id: String,
    kind: String,
    writers: Option<usize>,
    ops_per_writer: Option<usize>,
    client_exit7_retries: Option<u32>,
    expect_some_exit7: Option<bool>,
    busy_timeout_ms: Option<u64>,
    commit_retries: Option<u32>,
    ops: Option<usize>,
    iterations: Option<usize>,
}

struct MetricRow {
    phase: String,
    step: String,
    wall_ms: Option<f64>,
    exit7: u32,
    successes: u32,
    failures: u32,
    note: String,
}

struct PhaseStats {
    exit7_hits: u32,
    successes: u32,
    failures: u32,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn writer_count(phase: &Phase, meta: &Meta, heavy: bool) -> usize {
    let base = phase.writers.unwrap_or(meta.default_writers);
    if heavy {
        std::env::var("WISP_EXIT7_BATTERY_WRITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(meta.heavy_writers)
    } else {
        std::env::var("WISP_EXIT7_BATTERY_WRITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(base)
    }
}

fn arena_heading(prefix: &str, i: usize) -> String {
    format!("{prefix}{i}")
}

fn write_manifest_policy(root: &Path, busy_timeout_ms: u64, commit_retries: u32) {
    let path = root.join(".wisp/manifest.toml");
    let mut m = Manifest::load(&path).expect("manifest");
    m.commit.busy_timeout_ms = busy_timeout_ms;
    m.commit.retries = commit_retries;
    m.save(&path).expect("save manifest");
}

fn fresh_repo(meta: &Meta) -> (tempfile::TempDir, PathBuf, Repository) {
    let cat = catalog();
    let spec = cat.bundle(&meta.bundle_id).expect("bundle in catalog");
    let (dir, root, repo) = fresh_spy_repo(spec);
    write_manifest_policy(&root, meta.busy_timeout_ms, meta.commit_retries);
    drop(repo);
    let repo = Repository::open(&root, &unlock()).expect("reopen with policy");
    (dir, root, repo)
}

fn section_version(repo: &mut Repository, path: &str, heading: &str) -> (SectionId, ChangeId) {
    section_at(repo, path, heading)
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

fn apply_one(
    repo: &mut Repository,
    actor: &str,
    op: ApplyOp,
    message: &str,
) -> Result<serde_json::Value, WispError> {
    apply(
        repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some(actor.into()),
            message: Some(message.into()),
            ops: vec![op],
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        },
        None,
    )
}

fn apply_with_exit7_retries(
    repo: &mut Repository,
    actor: &str,
    mut op: ApplyOp,
    message: &str,
    client_retries: u32,
) -> (Result<serde_json::Value, WispError>, u32, Duration) {
    let t0 = Instant::now();
    let mut exit7_hits = 0u32;
    let max = client_retries + 1;
    let mut delay_ms = 15u64;
    let result = (|| {
        for attempt in 0..max {
            match apply_one(repo, actor, op.clone(), message) {
                Ok(v) => return Ok(v),
                Err(e) if e.code == ExitCode::WriteTimeout && attempt + 1 < max => {
                    exit7_hits += 1;
                    if let Some(section) = op.section {
                        if let Ok(hot) = repo.hot(MAIN_REF) {
                            if let Ok(Some(row)) = wisp_store::Hot::section(hot.conn(), section) {
                                op.base_version = Some(row.version.to_string());
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(delay_ms + (attempt as u64 * 7)));
                    delay_ms = (delay_ms * 2).min(120);
                    continue;
                }
                Err(e) if e.code == ExitCode::WriteTimeout => {
                    exit7_hits += 1;
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    })();
    (result, exit7_hits, t0.elapsed())
}

fn collect_arena_sections(
    repo: &mut Repository,
    meta: &Meta,
    n: usize,
) -> Vec<(SectionId, ChangeId)> {
    let path = &meta.arena_path;
    let prefix = &meta.arena_heading_prefix;
    (0..n)
        .map(|i| section_version(repo, path, &arena_heading(prefix, i)))
        .collect()
}

fn print_metrics(rows: &[MetricRow]) {
    eprintln!();
    eprintln!("battery-exit7-adversarial — metrics (ms)");
    eprintln!(
        "{:<26} {:<22} {:>10} {:>6} {:>5} {:>5} note",
        "phase", "step", "wall_ms", "exit7", "ok", "fail"
    );
    for r in rows {
        eprintln!(
            "{:<26} {:<22} {:>10} {:>6} {:>5} {:>5} {}",
            r.phase,
            r.step,
            match r.wall_ms {
                Some(x) => format!("{x:.3}"),
                None => "-".into(),
            },
            r.exit7,
            r.successes,
            r.failures,
            r.note
        );
    }
    eprintln!();
}

fn print_profile(agg: &BTreeMap<&'static str, (u128, usize)>) {
    eprintln!("commit/apply phase averages (ns per sample):");
    let mut total = 0u128;
    for (name, (sum, count)) in agg {
        let avg = *sum / *count as u128;
        if *name == "commit.total" || *name == "apply.total" {
            total = avg;
        }
        eprintln!("  {name:<32} {avg:>12} ns  (n={count})");
    }
    if total > 0 {
        eprintln!("  (reference total ~{:.3} ms)", total as f64 / 1e6);
    }
    eprintln!();
}

fn aggregate(samples: &[Vec<PhaseSample>]) -> BTreeMap<&'static str, (u128, usize)> {
    let mut acc: BTreeMap<&'static str, (u128, usize)> = BTreeMap::new();
    for run in samples {
        for s in run {
            let e = acc.entry(s.name).or_insert((0, 0));
            e.0 += s.nanos;
            e.1 += 1;
        }
    }
    acc
}

fn run_disjoint_parallel(
    meta: &Meta,
    phase: &Phase,
    heavy: bool,
    rows: &mut Vec<MetricRow>,
    stats: &mut PhaseStats,
) {
    let writers = writer_count(phase, meta, heavy);
    let ops = phase.ops_per_writer.unwrap_or(6);
    let client_retries = phase.client_exit7_retries.unwrap_or(0);
    let (_dir, root, repo) = fresh_repo(meta);
    let busy = phase.busy_timeout_ms.unwrap_or(meta.busy_timeout_ms);
    let retries = phase.commit_retries.unwrap_or(meta.commit_retries);
    write_manifest_policy(&root, busy, retries);
    drop(repo);
    let mut repo = Repository::open(&root, &unlock()).expect("policy reopen");
    let sections = collect_arena_sections(&mut repo, meta, writers);
    let path = meta.arena_path.clone();
    let prefix = meta.arena_heading_prefix.clone();
    drop(repo);

    let total_exit7 = Arc::new(AtomicU32::new(0));
    let total_ok = Arc::new(AtomicU32::new(0));
    let total_fail = Arc::new(AtomicU32::new(0));

    let handles: Vec<_> = sections
        .into_iter()
        .enumerate()
        .map(|(i, (sec, mut base))| {
            let root = root.clone();
            let path = path.clone();
            let _prefix = prefix.clone();
            let total_exit7 = Arc::clone(&total_exit7);
            let total_ok = Arc::clone(&total_ok);
            let total_fail = Arc::clone(&total_fail);
            std::thread::spawn(move || {
                let mut repo = Repository::open(&root, &unlock()).unwrap();
                let actor = format!("loq-explorer-{}/0.1", i + 1);
                let mut local_exit7 = 0u32;
                let mut local_ok = 0u32;
                let mut local_fail = 0u32;
                let t0 = Instant::now();
                for k in 0..ops {
                    let content = format!("explorer {} op {} — disjoint section write.\n", i, k);
                    let op = update_op(&path, sec, base, &content);
                    let (r, e7, _) = apply_with_exit7_retries(
                        &mut repo,
                        &actor,
                        op,
                        "exit7-battery",
                        client_retries,
                    );
                    local_exit7 += e7;
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
                            local_fail += 1;
                            if let Ok(hot) = repo.hot(MAIN_REF) {
                                if let Ok(Some(row)) = wisp_store::Hot::section(hot.conn(), sec) {
                                    base = row.version;
                                }
                            }
                        }
                        Err(e) => panic!("writer {i} op {k}: {e:?}"),
                    }
                }
                total_exit7.fetch_add(local_exit7, Ordering::Relaxed);
                total_ok.fetch_add(local_ok, Ordering::Relaxed);
                total_fail.fetch_add(local_fail, Ordering::Relaxed);
                (i, ms(t0.elapsed()), local_exit7, local_ok, local_fail)
            })
        })
        .collect();

    for h in handles {
        let (i, wall, e7, ok, fail) = h.join().unwrap();
        rows.push(MetricRow {
            phase: phase.id.clone(),
            step: format!("writer_{i}"),
            wall_ms: Some(wall),
            exit7: e7,
            successes: ok,
            failures: fail,
            note: format!("{ok}/{ops} ops ok"),
        });
    }

    let exit7 = total_exit7.load(Ordering::Relaxed);
    let ok = total_ok.load(Ordering::Relaxed);
    let fail = total_fail.load(Ordering::Relaxed);
    stats.exit7_hits += exit7;
    stats.successes += ok;
    stats.failures += fail;

    if phase.expect_some_exit7 == Some(true) {
        assert!(
            exit7 > 0 || fail > 0,
            "{}: expected measurable exit-7 pressure (exit7={exit7}, fail={fail})",
            phase.id
        );
    } else if client_retries == 0 {
        assert_eq!(
            fail, 0,
            "{}: engine retries should clear disjoint exit-7 (fail={fail}, exit7_hits={exit7})",
            phase.id
        );
    } else {
        assert_eq!(
            fail, 0,
            "{}: client+engine retries should complete all ops (fail={fail})",
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

fn run_batch_compare(meta: &Meta, phase: &Phase, rows: &mut Vec<MetricRow>) {
    let n = phase.ops.unwrap_or(5);
    let (_dir, _root, mut repo) = fresh_repo(meta);
    let path = meta.arena_path.clone();
    let (sec, mut ver) = section_version(
        &mut repo,
        &path,
        &arena_heading(&meta.arena_heading_prefix, 0),
    );

    let mut single_wall = 0f64;
    for i in 0..n {
        let content = format!("single-op batch compare {i}\n");
        let op = update_op(&path, sec, ver, &content);
        let t0 = Instant::now();
        let v = apply_one(&mut repo, "human:batch/1", op, "single").unwrap();
        single_wall += ms(t0.elapsed());
        ver = v["changes"][0]["version"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    }

    let ops: Vec<ApplyOp> = (0..n)
        .map(|i| {
            let heading = arena_heading(&meta.arena_heading_prefix, 2 + i);
            let (sec_i, base_i) = section_version(&mut repo, &path, &heading);
            update_op(&path, sec_i, base_i, &format!("batched body chunk {i}\n"))
        })
        .collect();
    let t0 = Instant::now();
    apply(
        &mut repo,
        ApplyBatch {
            r#ref: Some(MAIN_REF.into()),
            base: None,
            actor: Some("human:batch/1".into()),
            message: Some("batched".into()),
            ops,
            inputs: vec![],
            assert: vec![],
            assert_head: None,
            inputs_complete: false,
        },
        None,
    )
    .unwrap();
    let batch_wall = ms(t0.elapsed());

    rows.push(MetricRow {
        phase: phase.id.clone(),
        step: "single_ops".into(),
        wall_ms: Some(single_wall),
        exit7: 0,
        successes: n as u32,
        failures: 0,
        note: format!("{n} ledger commits"),
    });
    rows.push(MetricRow {
        phase: phase.id.clone(),
        step: "one_batch".into(),
        wall_ms: Some(batch_wall),
        exit7: 0,
        successes: 1,
        failures: 0,
        note: format!("1 commit, {n} ops"),
    });
    eprintln!(
        "E4 ratio single/batch wall: {:.2}x",
        single_wall / batch_wall.max(0.001)
    );
}

fn run_profile_sequential(meta: &Meta, phase: &Phase, rows: &mut Vec<MetricRow>) {
    let iterations = phase.iterations.unwrap_or(20);
    let (_dir, _root, mut repo) = fresh_repo(meta);
    let path = meta.arena_path.clone();
    let heading = arena_heading(&meta.arena_heading_prefix, 0);
    let (sec, mut ver) = section_version(&mut repo, &path, &heading);

    let mut runs: Vec<Vec<PhaseSample>> = Vec::new();
    for i in 0..iterations {
        let body = format!("profile iteration {i}\n");
        let op = update_op(&path, sec, ver, &body);
        let (res, phases) =
            with_profile(|| apply_one(&mut repo, "human:profile/1", op, &format!("p-{i}")));
        res.unwrap();
        runs.push(phases);
        ver = section_version(&mut repo, &path, &heading).1;
    }
    let agg = aggregate(&runs);
    print_profile(&agg);
    let avg_total = agg
        .get("apply.total")
        .map(|(s, c)| ms(Duration::from_nanos((*s / *c as u128) as u64)))
        .unwrap_or(0.0);
    rows.push(MetricRow {
        phase: phase.id.clone(),
        step: "steady_apply".into(),
        wall_ms: Some(avg_total),
        exit7: 0,
        successes: iterations as u32,
        failures: 0,
        note: "avg apply.total ms (see phase table above)".into(),
    });
}

fn run_phases(meta: &Meta, phases: &[Phase], heavy: bool) -> (Vec<MetricRow>, PhaseStats) {
    let mut rows = Vec::new();
    let mut stats = PhaseStats {
        exit7_hits: 0,
        successes: 0,
        failures: 0,
    };
    for phase in phases {
        match phase.kind.as_str() {
            "disjoint_parallel" => run_disjoint_parallel(meta, phase, heavy, &mut rows, &mut stats),
            "batch_compare" => run_batch_compare(meta, phase, &mut rows),
            "profile_sequential" => run_profile_sequential(meta, phase, &mut rows),
            other => panic!("unknown phase kind: {other}"),
        }
    }
    (rows, stats)
}

fn load_cases() -> CasesFile {
    toml::from_str(CASES_TOML).expect("parse cases.toml")
}

#[test]
fn battery_exit7_adversarial() {
    let cases = load_cases();
    let (rows, stats) = run_phases(&cases.meta, &cases.phase, false);
    print_metrics(&rows);
    eprintln!(
        "summary: exit7_hits={} successes={} terminal_failures={}",
        stats.exit7_hits, stats.successes, stats.failures
    );
}

#[test]
#[ignore = "heavy; run with WISP_EXIT7_BATTERY=1"]
fn battery_exit7_adversarial_heavy() {
    if std::env::var("WISP_EXIT7_BATTERY").as_deref() != Ok("1") {
        return;
    }
    let cases = load_cases();
    let (rows, stats) = run_phases(&cases.meta, &cases.phase, true);
    print_metrics(&rows);
    eprintln!(
        "heavy summary: exit7_hits={} successes={} terminal_failures={}",
        stats.exit7_hits, stats.successes, stats.failures
    );
}

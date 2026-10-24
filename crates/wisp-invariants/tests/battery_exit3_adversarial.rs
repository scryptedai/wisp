//! Adversarial exit-3 battery: `contrib/battery-exit3-adversarial/cases.toml`.

mod spy_harness;

use serde::Deserialize;
use spy_harness::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wisp_engine::headless::{apply_with_resolve, ApplyBatch, ApplyOp, ApplyResolvePolicy};
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::catalog;
use wisp_model::{ChangeId, ExitCode, SectionId, WispError};

const CASES_TOML: &str = include_str!("../../../contrib/battery-exit3-adversarial/cases.toml");

#[derive(Debug, Deserialize)]
struct CasesFile {
    meta: Meta,
    phase: Vec<Phase>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    bundle_id: String,
    frontier_path: String,
    frontier_heading: String,
    arena_path: String,
    arena_heading_prefix: String,
    default_writers: usize,
    heavy_writers: usize,
}

#[derive(Debug, Deserialize)]
struct Phase {
    id: String,
    kind: String,
    writers: Option<usize>,
    rounds: Option<u32>,
    ops_per_writer: Option<usize>,
    max_attempts: Option<u32>,
    waves: Option<u32>,
    max_retries: Option<u32>,
    good_writers: Option<usize>,
    poison_attempts: Option<u32>,
}

struct MetricRow {
    phase: String,
    step: String,
    exit3_ms: Option<f64>,
    resolve_ms: Option<f64>,
    exit7: u32,
    note: String,
}

struct PhaseStats {
    exit3_count: usize,
    success_count: usize,
    exit7_count: u32,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn writer_count(phase: &Phase, meta: &Meta, heavy: bool) -> usize {
    let base = phase.writers.unwrap_or(meta.default_writers);
    if heavy {
        std::env::var("WISP_EXIT3_BATTERY_WRITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(meta.heavy_writers)
    } else {
        std::env::var("WISP_EXIT3_BATTERY_WRITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(base)
    }
}

fn fresh_repo(meta: &Meta) -> (tempfile::TempDir, std::path::PathBuf, Repository) {
    let cat = catalog();
    let spec = cat.bundle(&meta.bundle_id).expect("bundle in catalog");
    let (dir, root, repo) = fresh_spy_repo(spec);
    assert_eq!(meta.bundle_id, spec.id);
    (dir, root, repo)
}

fn section_version(repo: &mut Repository, path: &str, heading: &str) -> (SectionId, ChangeId) {
    section_at(repo, path, heading)
}

fn stale_update(
    repo: &mut Repository,
    path: &str,
    section: SectionId,
    base: ChangeId,
    actor: &str,
    content: &str,
    message: &str,
) -> Result<serde_json::Value, WispError> {
    update_section_ref(
        repo,
        MAIN_REF,
        actor,
        path,
        section,
        base,
        content,
        message,
        vec![],
    )
}

fn resolve_update(
    repo: &mut Repository,
    path: &str,
    _heading: &str,
    section: SectionId,
    base: ChangeId,
    actor: &str,
    content: &str,
    message: &str,
    max_attempts: u32,
) -> Result<(serde_json::Value, usize, u32, Duration), WispError> {
    let t0 = Instant::now();
    let op = ApplyOp {
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
    };
    let batch = ApplyBatch {
        r#ref: Some(MAIN_REF.into()),
        base: None,
        actor: Some(actor.into()),
        message: Some(message.into()),
        ops: vec![op],
        inputs: vec![],
        assert: vec![],
        assert_head: None,
        inputs_complete: false,
    };
    let policy = ApplyResolvePolicy {
        max_attempts,
        conflict_sleep_ms: 5,
        retryable_sleep_ms: 15,
    };
    let (v, stats) = apply_with_resolve(repo, batch, None, &policy)?;
    Ok((
        v,
        stats.conflict_rounds as usize,
        stats.retryable_errors,
        t0.elapsed(),
    ))
}

fn arena_heading(prefix: &str, i: usize) -> String {
    format!("{prefix}{i}")
}

fn print_metrics(rows: &[MetricRow]) {
    eprintln!();
    eprintln!("battery-exit3-adversarial — metrics (ms)");
    eprintln!(
        "{:<22} {:<28} {:>10} {:>12} {:>6} {}",
        "phase", "step", "exit3_ms", "resolve_ms", "exit7", "note"
    );
    for r in rows {
        eprintln!(
            "{:<22} {:<28} {:>10} {:>12} {:>6} {}",
            r.phase,
            r.step,
            fmt_opt(r.exit3_ms),
            fmt_opt(r.resolve_ms),
            r.exit7,
            r.note
        );
    }
    eprintln!();
}

fn fmt_opt(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{:.3}", x),
        None => "-".into(),
    }
}

fn run_phases(meta: &Meta, phases: &[Phase], heavy: bool) -> (Vec<MetricRow>, PhaseStats) {
    let mut rows = Vec::new();
    let mut stats = PhaseStats {
        exit3_count: 0,
        success_count: 0,
        exit7_count: 0,
    };

    for phase in phases {
        match phase.kind.as_str() {
            "stale_stampede" => {
                let n = writer_count(phase, meta, heavy);
                let rounds = phase.rounds.unwrap_or(1);
                for r in 0..rounds {
                    let (_dir, root, mut repo) = fresh_repo(meta);
                    let (sec, stale) =
                        section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                    // Advance head so every parallel writer's captured base is genuinely stale.
                    let fresh =
                        section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading).1;
                    stale_update(
                        &mut repo,
                        &meta.frontier_path,
                        sec,
                        fresh,
                        "human:calibrator/1",
                        "pre-stampede bump\n",
                        "bump",
                    )
                    .unwrap();
                    drop(repo);
                    let stale = stale;
                    let path = meta.frontier_path.clone();
                    let handles: Vec<_> = (0..n)
                        .map(|w| {
                            let root = root.clone();
                            let path = path.clone();
                            std::thread::spawn(move || {
                                let mut repo =
                                    Repository::open(&root, &spy_harness::unlock()).unwrap();
                                let t0 = Instant::now();
                                let res = stale_update(
                                    &mut repo,
                                    &path,
                                    sec,
                                    stale,
                                    &format!("process:stampede-{w}"),
                                    &format!("stampede writer {w} round {r}\n"),
                                    "stampede",
                                );
                                (res, t0.elapsed(), w)
                            })
                        })
                        .collect();
                    let mut conflicts = 0usize;
                    for h in handles {
                        let (res, elapsed, w) = h.join().unwrap();
                        match res {
                            Err(e) if e.code == ExitCode::Conflict => {
                                conflicts += 1;
                                stats.exit3_count += 1;
                                rows.push(MetricRow {
                                    phase: phase.id.clone(),
                                    step: format!("stale_w{w}_r{r}"),
                                    exit3_ms: Some(ms(elapsed)),
                                    resolve_ms: None,
                                    exit7: 0,
                                    note: "expected exit 3".into(),
                                });
                            }
                            Ok(_) => {
                                stats.success_count += 1;
                                rows.push(MetricRow {
                                    phase: phase.id.clone(),
                                    step: format!("unexpected_ok_w{w}"),
                                    exit3_ms: None,
                                    resolve_ms: None,
                                    exit7: 0,
                                    note: "FAIL stale should not commit".into(),
                                });
                            }
                            Err(e) => panic!("stampede: {:?}", e),
                        }
                    }
                    assert_eq!(
                        conflicts, n,
                        "A1: every writer must exit 3 (got {conflicts}/{n})"
                    );
                }
            }
            "parallel_resolve" => {
                let n = writer_count(phase, meta, heavy);
                let max = phase.max_attempts.unwrap_or(24);
                let (_dir, root, mut repo) = fresh_repo(meta);
                let (sec, stale) =
                    section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                let path = meta.frontier_path.clone();
                let heading = meta.frontier_heading.clone();
                drop(repo);
                let successes = Arc::new(AtomicUsize::new(0));
                let handles: Vec<_> = (0..n)
                    .map(|w| {
                        let root = root.clone();
                        let path = path.clone();
                        let heading = heading.clone();
                        let successes = successes.clone();
                        std::thread::spawn(move || {
                            let mut repo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                            let (v, conflicts, exit7, elapsed) = resolve_update(
                                &mut repo,
                                &path,
                                &heading,
                                sec,
                                stale,
                                &format!("process:resolve-{w}"),
                                &format!("resolved writer {w}\n"),
                                "resolve",
                                max,
                            )
                            .unwrap();
                            successes.fetch_add(1, Ordering::SeqCst);
                            (
                                v,
                                conflicts,
                                exit7,
                                elapsed,
                                w,
                                stale_update(
                                    &mut repo,
                                    &path,
                                    sec,
                                    stale,
                                    &format!("process:resolve-{w}"),
                                    "noop",
                                    "noop",
                                ),
                            )
                        })
                    })
                    .collect();
                let mut total_conflicts = 0usize;
                for h in handles {
                    let (_v, conflicts, exit7, elapsed, w, stale_check) = h.join().unwrap();
                    total_conflicts += conflicts;
                    stats.exit3_count += conflicts;
                    stats.exit7_count += exit7;
                    stats.success_count += 1;
                    assert!(
                        stale_check.is_err_and(|e| e.code == ExitCode::Conflict),
                        "stale base must still conflict after resolve"
                    );
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        step: format!("resolve_w{w}"),
                        exit3_ms: None,
                        resolve_ms: Some(ms(elapsed)),
                        exit7: exit7,
                        note: format!("{conflicts} conflicts before ok"),
                    });
                }
                assert_eq!(successes.load(Ordering::SeqCst), n);
                assert!(
                    total_conflicts >= n,
                    "each writer should have hit >=1 conflict"
                );
            }
            "disjoint_parallel" => {
                let n = writer_count(phase, meta, heavy);
                let ops = phase.ops_per_writer.unwrap_or(2);
                let (_dir, root, mut repo) = fresh_repo(meta);
                let prefix = meta.arena_heading_prefix.clone();
                let path = meta.arena_path.clone();
                let mut sections = Vec::new();
                for i in 0..n {
                    let h = arena_heading(&prefix, i);
                    sections.push(section_version(&mut repo, &path, &h));
                }
                drop(repo);
                let mut conflicts = 0usize;
                let handles: Vec<_> = sections
                    .into_iter()
                    .enumerate()
                    .map(|(i, (sec, mut base))| {
                        let root = root.clone();
                        let path = path.clone();
                        let prefix = prefix.clone();
                        std::thread::spawn(move || {
                            let mut repo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                            let mut local_conflicts = 0usize;
                            for k in 0..ops {
                                let t0 = Instant::now();
                                let content = format!("disjoint {i} op {k}\n");
                                let r = stale_update(
                                    &mut repo,
                                    &path,
                                    sec,
                                    base,
                                    &format!("process:d{i}"),
                                    &content,
                                    "disjoint",
                                );
                                let elapsed = t0.elapsed();
                                match r {
                                    Ok(_) => {
                                        base = section_version(
                                            &mut repo,
                                            &path,
                                            &arena_heading(&prefix, i),
                                        )
                                        .1;
                                    }
                                    Err(e) if e.code == ExitCode::Conflict => {
                                        local_conflicts += 1;
                                        base = section_version(
                                            &mut repo,
                                            &path,
                                            &arena_heading(&prefix, i),
                                        )
                                        .1;
                                    }
                                    Err(e) => panic!("disjoint: {e:?}"),
                                }
                                if local_conflicts > 0 {
                                    return (local_conflicts, elapsed, i);
                                }
                            }
                            (local_conflicts, Duration::ZERO, i)
                        })
                    })
                    .collect();
                for h in handles {
                    let (c, _, i) = h.join().unwrap();
                    conflicts += c;
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        step: format!("disjoint_s{i}"),
                        exit3_ms: None,
                        resolve_ms: None,
                        exit7: 0,
                        note: if c == 0 {
                            "0 exit 3".into()
                        } else {
                            format!("unexpected {c} exit3")
                        },
                    });
                }
                assert_eq!(conflicts, 0, "B1: disjoint sections must not exit 3");
                let mut repo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                assert!(wisp_engine::query::fsck(&mut repo).unwrap()["ok"]
                    .as_bool()
                    .unwrap());
            }
            "append_stale_update" => {
                let rounds = phase.rounds.unwrap_or(1);
                let (_dir, _root, mut repo) = fresh_repo(meta);
                let (sec, mut ver) =
                    section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                for r in 0..rounds {
                    patch_section(
                        &mut repo,
                        "human:log/1",
                        &meta.frontier_path,
                        sec,
                        "append_section",
                        &format!("log line {r}\n"),
                        "append",
                    )
                    .unwrap();
                    let stale = ver;
                    let t0 = Instant::now();
                    let err = stale_update(
                        &mut repo,
                        &meta.frontier_path,
                        sec,
                        stale,
                        "human:editor/1",
                        "full replace attempt\n",
                        "stale",
                    )
                    .unwrap_err();
                    assert_eq!(err.code, ExitCode::Conflict);
                    stats.exit3_count += 1;
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        step: format!("stale_r{r}"),
                        exit3_ms: Some(ms(t0.elapsed())),
                        resolve_ms: None,
                        exit7: 0,
                        note: "append then stale update".into(),
                    });
                    let fresh =
                        section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading).1;
                    stale_update(
                        &mut repo,
                        &meta.frontier_path,
                        sec,
                        fresh,
                        "human:editor/1",
                        "full replace ok\n",
                        "resolve",
                    )
                    .unwrap();
                    ver = section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading).1;
                }
            }
            "stale_then_blind_append" => {
                let rounds = phase.rounds.unwrap_or(1);
                let (_dir, _root, mut repo) = fresh_repo(meta);
                let (sec, stale) =
                    section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                for r in 0..rounds {
                    let fresh =
                        section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading).1;
                    stale_update(
                        &mut repo,
                        &meta.frontier_path,
                        sec,
                        fresh,
                        "human:calibrator/1",
                        &format!("round {r} bump\n"),
                        "bump",
                    )
                    .unwrap();
                    let t0 = Instant::now();
                    let err = stale_update(
                        &mut repo,
                        &meta.frontier_path,
                        sec,
                        stale,
                        "human:editor/1",
                        "nope\n",
                        "stale",
                    )
                    .unwrap_err();
                    assert_eq!(err.code, ExitCode::Conflict);
                    stats.exit3_count += 1;
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        step: format!("stale_r{r}"),
                        exit3_ms: Some(ms(t0.elapsed())),
                        resolve_ms: None,
                        exit7: 0,
                        note: "stale blocked".into(),
                    });
                    patch_section(
                        &mut repo,
                        "human:log/1",
                        &meta.frontier_path,
                        sec,
                        "append_section",
                        &format!("still logging {r}\n"),
                        "append",
                    )
                    .unwrap();
                }
            }
            "retry_budget" => {
                let n = writer_count(phase, meta, heavy);
                let waves = phase.waves.unwrap_or(1);
                let max = phase.max_retries.unwrap_or(12);
                for wave in 0..waves {
                    let (_dir, root, mut repo) = fresh_repo(meta);
                    let (sec, stale) =
                        section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                    let path = meta.frontier_path.clone();
                    let heading = meta.frontier_heading.clone();
                    drop(repo);
                    for w in 0..n {
                        let mut rrepo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                        let (_v, conflicts, exit7, elapsed) = resolve_update(
                            &mut rrepo,
                            &path,
                            &heading,
                            sec,
                            stale,
                            &format!("process:budget-{w}"),
                            &format!("wave {wave} writer {w}\n"),
                            "budget",
                            max,
                        )
                        .unwrap();
                        stats.exit3_count += conflicts;
                        stats.exit7_count += exit7;
                        stats.success_count += 1;
                        rows.push(MetricRow {
                            phase: phase.id.clone(),
                            step: format!("wave{wave}_w{w}"),
                            exit3_ms: None,
                            resolve_ms: Some(ms(elapsed)),
                            exit7,
                            note: format!("{conflicts} conflicts"),
                        });
                    }
                }
            }
            "poison_neighbor" => {
                let good = phase.good_writers.unwrap_or(4);
                let poison_tries = phase.poison_attempts.unwrap_or(8);
                let (_dir, root, mut repo) = fresh_repo(meta);
                let (sec, stale) =
                    section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading);
                let path = meta.frontier_path.clone();
                let prefix = meta.arena_heading_prefix.clone();
                let arena = meta.arena_path.clone();
                let mut good_secs = Vec::new();
                for i in 0..good {
                    let h = arena_heading(&prefix, i);
                    good_secs.push(section_version(&mut repo, &arena, &h));
                }
                let fresh =
                    section_version(&mut repo, &meta.frontier_path, &meta.frontier_heading).1;
                stale_update(
                    &mut repo,
                    &meta.frontier_path,
                    sec,
                    fresh,
                    "human:calibrator/1",
                    "poison isolation bump\n",
                    "bump",
                )
                .unwrap();
                drop(repo);
                let poison = std::thread::spawn({
                    let root = root.clone();
                    let path = path.clone();
                    move || {
                        let mut repo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                        let mut poison_conflicts = 0usize;
                        for _ in 0..poison_tries {
                            let t0 = Instant::now();
                            if stale_update(
                                &mut repo,
                                &path,
                                sec,
                                stale,
                                "process:poison",
                                "never wins\n",
                                "poison",
                            )
                            .is_err_and(|e| e.code == ExitCode::Conflict)
                            {
                                poison_conflicts += 1;
                                let _ = t0.elapsed();
                            }
                        }
                        poison_conflicts
                    }
                });
                let good_handles: Vec<_> = good_secs
                    .into_iter()
                    .enumerate()
                    .map(|(i, (gsec, gbase))| {
                        let root = root.clone();
                        let arena = arena.clone();
                        std::thread::spawn(move || {
                            let mut repo = Repository::open(&root, &spy_harness::unlock()).unwrap();
                            stale_update(
                                &mut repo,
                                &arena,
                                gsec,
                                gbase,
                                &format!("process:good-{i}"),
                                &format!("good {i}\n"),
                                "good",
                            )
                            .is_ok()
                        })
                    })
                    .collect();
                let poison_n = poison.join().unwrap();
                assert_eq!(poison_n, poison_tries as usize, "poison must always exit 3");
                stats.exit3_count += poison_n;
                let mut good_ok = 0usize;
                for (i, h) in good_handles.into_iter().enumerate() {
                    assert!(h.join().unwrap(), "good writer {i} must succeed");
                    good_ok += 1;
                }
                stats.success_count += good_ok;
                rows.push(MetricRow {
                    phase: phase.id.clone(),
                    step: "poison".into(),
                    exit3_ms: None,
                    resolve_ms: None,
                    exit7: 0,
                    note: format!("{poison_n} exit3, poison isolated"),
                });
            }
            other => panic!("unknown phase kind: {other}"),
        }
    }
    (rows, stats)
}

fn load_cases() -> CasesFile {
    toml::from_str(CASES_TOML).expect("parse cases.toml")
}

#[test]
fn battery_exit3_adversarial() {
    let cases = load_cases();
    let (rows, stats) = run_phases(&cases.meta, &cases.phase, false);
    print_metrics(&rows);
    eprintln!(
        "summary: exit3={} successes={} exit7={}",
        stats.exit3_count, stats.success_count, stats.exit7_count
    );
    assert!(
        stats.exit3_count >= cases.phase.len(),
        "expected substantial exit-3 samples"
    );
    assert!(stats.success_count > 0, "expected successful resolutions");
}

#[test]
#[ignore = "heavy; run with WISP_EXIT3_BATTERY=1"]
fn battery_exit3_adversarial_heavy() {
    if std::env::var("WISP_EXIT3_BATTERY").as_deref() != Ok("1") {
        return;
    }
    let cases = load_cases();
    let (rows, stats) = run_phases(&cases.meta, &cases.phase, true);
    print_metrics(&rows);
    eprintln!(
        "heavy summary: exit3={} successes={} exit7={}",
        stats.exit3_count, stats.success_count, stats.exit7_count
    );
}

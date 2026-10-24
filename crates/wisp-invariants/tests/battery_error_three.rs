//! Exit-3 / merge timing battery (section 1): `contrib/battery-error-three/cases.toml`.

mod spy_harness;

use serde::Deserialize;
use spy_harness::*;
use std::time::{Duration, Instant};
use wisp_engine::merge::merge_refs;
use wisp_engine::query::branch;
use wisp_engine::repo::{Repository, MAIN_REF};
use wisp_invariants::catalog;
use wisp_model::{ExitCode, WispError};

const CASES_TOML: &str = include_str!("../../../contrib/battery-error-three/cases.toml");

#[derive(Debug, Deserialize)]
struct CasesFile {
    meta: Meta,
    phase: Vec<Phase>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    bundle_id: String,
    concept_path: String,
}

#[derive(Debug, Deserialize)]
struct Phase {
    id: String,
    kind: String,
    actor: Option<String>,
    section: Option<String>,
    actor_a: Option<String>,
    actor_b: Option<String>,
    actors: Option<Vec<String>>,
    sections: Option<Vec<String>>,
    branch_prefix: Option<String>,
    round: Vec<PhaseRound>,
}

#[derive(Debug, Deserialize)]
struct PhaseRound {
    n: u32,
    message: Option<String>,
    content: Option<String>,
    message_a: Option<String>,
    message_b: Option<String>,
    content_a: Option<String>,
    content_b: Option<String>,
    content_alpha: Option<String>,
    content_beta: Option<String>,
    content_gamma: Option<String>,
    fragment: Option<String>,
    fragment_a: Option<String>,
    fragment_b: Option<String>,
}

struct MetricRow {
    phase: String,
    round: u32,
    step: String,
    append_ms: Option<f64>,
    conflict_detect_ms: Option<f64>,
    resolution_ms: Option<f64>,
    note: String,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fresh_battery_repo() -> (tempfile::TempDir, Repository) {
    let cat = catalog();
    let spec = cat
        .bundle("battery-error-three")
        .expect("battery-error-three bundle");
    let (dir, _root, repo) = fresh_spy_repo(spec);
    (dir, repo)
}

fn timed_update(
    repo: &mut Repository,
    ref_name: &str,
    actor: &str,
    path: &str,
    section: wisp_model::SectionId,
    base: wisp_model::ChangeId,
    content: &str,
    message: &str,
) -> (Result<serde_json::Value, WispError>, Duration) {
    let t0 = Instant::now();
    let r = update_section_ref(
        repo,
        ref_name,
        actor,
        path,
        section,
        base,
        content,
        message,
        vec![],
    );
    (r, t0.elapsed())
}

fn timed_patch(
    repo: &mut Repository,
    ref_name: &str,
    actor: &str,
    path: &str,
    section: wisp_model::SectionId,
    op: &str,
    fragment: &str,
    message: &str,
) -> (Result<serde_json::Value, WispError>, Duration) {
    let t0 = Instant::now();
    let r = patch_section_ref(repo, ref_name, actor, path, section, op, fragment, message);
    (r, t0.elapsed())
}

fn print_metrics(rows: &[MetricRow]) {
    eprintln!();
    eprintln!("battery-error-three section 1 — timings (ms)");
    eprintln!(
        "{:<18} {:>5} {:<28} {:>12} {:>18} {:>14} {}",
        "phase", "round", "step", "append_ms", "conflict_detect_ms", "resolution_ms", "note"
    );
    for r in rows {
        eprintln!(
            "{:<18} {:>5} {:<28} {:>12} {:>18} {:>14} {}",
            r.phase,
            r.round,
            r.step,
            fmt_opt(r.append_ms),
            fmt_opt(r.conflict_detect_ms),
            fmt_opt(r.resolution_ms),
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

fn load_cases() -> CasesFile {
    toml::from_str(CASES_TOML).expect("parse cases.toml")
}

#[test]
fn battery_error_three_section_one() {
    let cases = load_cases();
    assert_eq!(cases.meta.bundle_id, "battery-error-three");
    let concept_path = cases.meta.concept_path.clone();

    let mut rows: Vec<MetricRow> = Vec::new();

    for phase in &cases.phase {
        match phase.kind.as_str() {
            "single_sequential" => {
                let actor = phase.actor.as_deref().expect("actor");
                let section_name = phase.section.as_deref().expect("section");
                let (_dir, mut repo) = fresh_battery_repo();
                let (sec, mut ver) = section_at(&mut repo, &concept_path, section_name);
                for round in &phase.round {
                    let content = round.content.as_deref().expect("content");
                    let message = round.message.as_deref().expect("message");
                    let (res, elapsed) = timed_update(
                        &mut repo,
                        MAIN_REF,
                        actor,
                        &concept_path,
                        sec,
                        ver,
                        content,
                        message,
                    );
                    res.expect("single append");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "append".into(),
                        append_ms: Some(ms(elapsed)),
                        conflict_detect_ms: None,
                        resolution_ms: None,
                        note: "solo Frontier".into(),
                    });
                    (_, ver) = section_at(&mut repo, &concept_path, section_name);
                }
            }
            "dual_same_section" => {
                let section_name = phase.section.as_deref().expect("section");
                let actor_a = phase.actor_a.as_deref().expect("actor_a");
                let actor_b = phase.actor_b.as_deref().expect("actor_b");
                for round in &phase.round {
                    let (_dir, mut repo) = fresh_battery_repo();
                    let (sec, base) = section_at(&mut repo, &concept_path, section_name);
                    let content_a = round.content_a.as_deref().expect("content_a");
                    let content_b = round.content_b.as_deref().expect("content_b");
                    let message_a = round.message_a.as_deref().expect("message_a");
                    let message_b = round.message_b.as_deref().expect("message_b");

                    let (res_a, t_a) = timed_update(
                        &mut repo,
                        MAIN_REF,
                        actor_a,
                        &concept_path,
                        sec,
                        base,
                        content_a,
                        message_a,
                    );
                    res_a.expect("actor_a append");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "append_a".into(),
                        append_ms: Some(ms(t_a)),
                        conflict_detect_ms: None,
                        resolution_ms: None,
                        note: "explorer-1 wins race".into(),
                    });

                    let (res_b, t_b) = timed_update(
                        &mut repo,
                        MAIN_REF,
                        actor_b,
                        &concept_path,
                        sec,
                        base,
                        content_b,
                        message_b,
                    );
                    let err = res_b.expect_err("stale base conflict");
                    assert_eq!(err.code, ExitCode::Conflict);
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "conflict_b".into(),
                        append_ms: None,
                        conflict_detect_ms: Some(ms(t_b)),
                        resolution_ms: None,
                        note: "exit 3 stale base".into(),
                    });

                    let (_, fresh_base) = section_at(&mut repo, &concept_path, section_name);
                    let (res_fix, t_fix) = timed_update(
                        &mut repo,
                        MAIN_REF,
                        actor_b,
                        &concept_path,
                        sec,
                        fresh_base,
                        content_b,
                        message_b,
                    );
                    res_fix.expect("actor_b resolution");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "resolve_b".into(),
                        append_ms: None,
                        conflict_detect_ms: None,
                        resolution_ms: Some(ms(t_fix)),
                        note: "fresh base retry".into(),
                    });
                }
            }
            "blind_append_sequential" => {
                let actor = phase.actor.as_deref().expect("actor");
                let section_name = phase.section.as_deref().expect("section");
                let (_dir, mut repo) = fresh_battery_repo();
                let (sec, _) = section_at(&mut repo, &concept_path, section_name);
                for round in &phase.round {
                    let fragment = round.fragment.as_deref().expect("fragment");
                    let message = round.message.as_deref().expect("message");
                    let (res, elapsed) = timed_patch(
                        &mut repo,
                        MAIN_REF,
                        actor,
                        &concept_path,
                        sec,
                        "append_section",
                        fragment,
                        message,
                    );
                    res.expect("blind append");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "append_section".into(),
                        append_ms: Some(ms(elapsed)),
                        conflict_detect_ms: None,
                        resolution_ms: None,
                        note: "blind append_section".into(),
                    });
                }
            }
            "blind_append_dual" => {
                let section_name = phase.section.as_deref().expect("section");
                let actor_a = phase.actor_a.as_deref().expect("actor_a");
                let actor_b = phase.actor_b.as_deref().expect("actor_b");
                for round in &phase.round {
                    let (_dir, mut repo) = fresh_battery_repo();
                    let (sec, _) = section_at(&mut repo, &concept_path, section_name);
                    let frag_a = round.fragment_a.as_deref().expect("fragment_a");
                    let frag_b = round.fragment_b.as_deref().expect("fragment_b");
                    let msg_a = round.message_a.as_deref().expect("message_a");
                    let msg_b = round.message_b.as_deref().expect("message_b");

                    let (res_a, t_a) = timed_patch(
                        &mut repo,
                        MAIN_REF,
                        actor_a,
                        &concept_path,
                        sec,
                        "append_section",
                        frag_a,
                        msg_a,
                    );
                    res_a.expect("actor_a blind append");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "append_a".into(),
                        append_ms: Some(ms(t_a)),
                        conflict_detect_ms: None,
                        resolution_ms: None,
                        note: "blind append explorer-1".into(),
                    });

                    let (res_b, t_b) = timed_patch(
                        &mut repo,
                        MAIN_REF,
                        actor_b,
                        &concept_path,
                        sec,
                        "append_section",
                        frag_b,
                        msg_b,
                    );
                    res_b.expect("actor_b blind append (no exit 3)");
                    rows.push(MetricRow {
                        phase: phase.id.clone(),
                        round: round.n,
                        step: "append_b".into(),
                        append_ms: Some(ms(t_b)),
                        conflict_detect_ms: None,
                        resolution_ms: None,
                        note: "blind append explorer-2".into(),
                    });
                }
            }
            "triple_independent_merge" => {
                let actors = phase.actors.as_ref().expect("actors");
                let sections = phase.sections.as_ref().expect("sections");
                let prefix = phase.branch_prefix.as_deref().unwrap_or("swarm");
                assert_eq!(actors.len(), 3);
                assert_eq!(sections.len(), 3);

                for round in &phase.round {
                    let (_dir, mut repo) = fresh_battery_repo();
                    let content_alpha = round.content_alpha.as_deref().expect("content_alpha");
                    let content_beta = round.content_beta.as_deref().expect("content_beta");
                    let content_gamma = round.content_gamma.as_deref().expect("content_gamma");
                    let msg = round.message.as_deref().unwrap_or("triple lane update");

                    let branches = [
                        format!("{prefix}/r{}/a", round.n),
                        format!("{prefix}/r{}/b", round.n),
                        format!("{prefix}/r{}/c", round.n),
                    ];
                    let contents = [content_alpha, content_beta, content_gamma];

                    for i in 0..3 {
                        branch(&mut repo, &branches[i], None).unwrap();
                        let (sec, ver) =
                            section_at_ref(&mut repo, &branches[i], &concept_path, &sections[i]);
                        let (res, t) = timed_update(
                            &mut repo,
                            &branches[i],
                            &actors[i],
                            &concept_path,
                            sec,
                            ver,
                            contents[i],
                            msg,
                        );
                        res.expect("branch append");
                        rows.push(MetricRow {
                            phase: phase.id.clone(),
                            round: round.n,
                            step: format!("append_{}", ['a', 'b', 'c'][i]),
                            append_ms: Some(ms(t)),
                            conflict_detect_ms: None,
                            resolution_ms: None,
                            note: sections[i].clone(),
                        });
                    }

                    for i in 0..3 {
                        let t0 = Instant::now();
                        let merged =
                            merge_refs(&mut repo, &branches[i], MAIN_REF, None, false).unwrap();
                        let elapsed = t0.elapsed();
                        let status = merged["status"].as_str().unwrap_or("?");
                        rows.push(MetricRow {
                            phase: phase.id.clone(),
                            round: round.n,
                            step: format!("merge_{}", ['a', 'b', 'c'][i]),
                            append_ms: None,
                            conflict_detect_ms: None,
                            resolution_ms: Some(ms(elapsed)),
                            note: format!("{} → main ({})", branches[i], status),
                        });
                    }
                }
            }
            other => panic!("unknown phase kind: {other}"),
        }
    }

    print_metrics(&rows);
    assert!(
        rows.iter().any(|r| r.conflict_detect_ms.is_some()),
        "expected conflict_detect_ms samples"
    );
    assert!(
        rows.iter().filter(|r| r.resolution_ms.is_some()).count() >= 10,
        "expected merge + resolve samples"
    );
}

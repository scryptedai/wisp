//! Deep timing breakdown for `headless::apply` / `update_section` (battery-error-three).

mod spy_harness;

use spy_harness::*;
use std::collections::BTreeMap;
use wisp_engine::headless::{apply, ApplyBatch, ApplyOp};
use wisp_engine::repo::MAIN_REF;
use wisp_engine::timing::{with_profile, PhaseSample};
use wisp_invariants::catalog;
use wisp_model::{ChangeId, SectionId};

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

fn apply_update(
    repo: &mut wisp_engine::Repository,
    actor: &str,
    concept_path: &str,
    section: SectionId,
    base: ChangeId,
    content: &str,
    message: &str,
) -> Result<serde_json::Value, wisp_model::WispError> {
    let op = ApplyOp {
        op: "update_section".into(),
        path: format!("{concept_path}.md"),
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

#[test]
fn append_profile_battery_error_three() {
    let cat = catalog();
    let spec = cat.bundle("battery-error-three").expect("bundle");
    let (dir, _root, mut repo) = fresh_spy_repo(spec);
    let concept_path = "notes/swarm-metrics";
    let (sec, mut ver) = section_at(&mut repo, concept_path, "Frontier");

    // Warm-up: register actor + one append (not counted).
    let warmup = "Warm-up append.\n";
    apply_update(
        &mut repo,
        "human:calibrator/1",
        concept_path,
        sec,
        ver,
        warmup,
        "warmup",
    )
    .unwrap();
    (_, ver) = section_at(&mut repo, concept_path, "Frontier");

    let iterations = 30;
    let mut runs: Vec<Vec<PhaseSample>> = Vec::new();
    for i in 0..iterations {
        let body = format!("Profile iteration {i} — synthetic Frontier body.\n");
        let (res, phases) = with_profile(|| {
            apply_update(
                &mut repo,
                "human:calibrator/1",
                concept_path,
                sec,
                ver,
                &body,
                &format!("profile-{i}"),
            )
        });
        res.unwrap();
        runs.push(phases);
        (_, ver) = section_at(&mut repo, concept_path, "Frontier");
    }

    let agg = aggregate(&runs);
    let total_ns: u128 = agg
        .get("apply.total")
        .map(|(s, c)| *s / *c as u128)
        .unwrap_or(0);

    eprintln!();
    eprintln!(
        "append profile — battery-error-three, {iterations} steady-state appends on main (debug build)"
    );
    eprintln!("repo: {}", dir.path().display());
    eprintln!(
        "{:<36} {:>12} {:>12} {:>8}",
        "phase", "mean_ms", "mean_us", "pct"
    );
    for (name, (sum, count)) in &agg {
        let mean_ns = *sum / *count as u128;
        let pct = if total_ns > 0 {
            (mean_ns as f64 / total_ns as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "{:<36} {:>12.3} {:>12.1} {:>7.1}%",
            name,
            mean_ns as f64 / 1_000_000.0,
            mean_ns as f64 / 1_000.0,
            pct
        );
    }
    eprintln!();
    eprintln!(
        "apply.total mean: {:.3} ms ({:.0} µs)",
        total_ns as f64 / 1_000_000.0,
        total_ns as f64 / 1_000.0
    );

    // Sanity: worktree refresh should dominate if full-tree render runs every append.
    let wt = agg
        .get("worktree.write_tree")
        .map(|(s, c)| *s / *c as u128)
        .unwrap_or(0);
    let commit = agg
        .get("commit.total")
        .map(|(s, c)| *s / *c as u128)
        .unwrap_or(0);
    eprintln!(
        "worktree.write_tree mean {:.3} ms vs commit.total mean {:.3} ms",
        wt as f64 / 1_000_000.0,
        commit as f64 / 1_000_000.0
    );
}

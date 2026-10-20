//! Corpus-driven tests (§17). Add fixtures under `wisp-corpus/data/` and entries in `catalog.toml`.

use wisp_corpus::load_match_scenario;
use wisp_engine::headless::ApplyOp;
use wisp_engine::repo::{Unlock, MAIN_REF};
use wisp_invariants::{
    assert_lossless_strip, catalog, init_and_record_bundle, must_findings,
};
use wisp_match::{match_sections, Context, Decision, PriorSection};
use wisp_model::{ConceptId, SectionId};
use wisp_parse::parse_file;

fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

#[test]
fn catalog_loads_and_paths_exist() {
    let c = catalog();
    assert!(!c.bundle.is_empty());
    for b in &c.bundle {
        let root = wisp_corpus::bundle_root(b);
        assert!(root.is_dir(), "bundle {} missing at {}", b.id, root.display());
    }
    for concept in &c.concept {
        let p = wisp_corpus::data_dir().join(&concept.path);
        assert!(p.is_file(), "concept {} missing", concept.id);
    }
    for s in &c.scenario {
        let p = wisp_corpus::data_dir().join(&s.path).join("scenario.toml");
        assert!(p.is_file(), "scenario {} missing", s.id);
    }
}

#[test]
fn corpus_concepts_parse() {
    let c = catalog();
    for spec in &c.concept {
        let text = wisp_corpus::read_concept(spec).unwrap();
        let parsed = parse_file(&text);
        if spec.expect_frontmatter == Some(false) {
            assert!(
                parsed.frontmatter.is_none(),
                "{} should have no frontmatter",
                spec.id
            );
        }
        if spec.expect_opaque_frontmatter == Some(true) {
            assert!(
                parsed.frontmatter.as_ref().unwrap().opaque,
                "{} should be opaque frontmatter",
                spec.id
            );
        }
        if let Some(n) = spec.expect_min_sections {
            assert!(
                parsed.sections().len() >= n,
                "{} expected >={} sections, got {}",
                spec.id,
                n,
                parsed.sections().len()
            );
        }
        if let Some(n) = spec.expect_max_sections {
            assert!(
                parsed.sections().len() <= n,
                "{} expected <={} sections, got {}",
                spec.id,
                n,
                parsed.sections().len()
            );
        }
        if let Some(n) = spec.expect_erasure_markers {
            assert_eq!(
                parsed.sectioned.erasure_markers.len(),
                n,
                "{} erasure markers",
                spec.id
            );
        }
        if let Some(needles) = &spec.expect_warning_contains {
            for needle in needles {
                assert!(
                    parsed.sectioned.warnings.iter().any(|w| w.contains(needle)),
                    "{} missing warning containing {:?}, got {:?}",
                    spec.id,
                    needle,
                    parsed.sectioned.warnings
                );
            }
        }
    }
}

struct ScenarioCtx {
    erased: std::collections::HashSet<SectionId>,
}

impl ScenarioCtx {
    fn from_spec(ctx: &wisp_corpus::ScenarioCtx) -> Self {
        ScenarioCtx {
            erased: ctx
                .erased_sections
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect(),
        }
    }
}

impl Context for ScenarioCtx {
    fn owner_of(&self, _: SectionId) -> Option<ConceptId> {
        None
    }
    fn owner_still_has(&self, _: SectionId) -> bool {
        false
    }
    fn is_erased(&self, s: SectionId) -> bool {
        self.erased.contains(&s)
    }
}

#[test]
fn corpus_bundles_record_and_validate() {
    for spec in catalog().bundle.iter().filter(|b| b.record) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundle");
        let before =
            wisp_corpus::snapshot_bundle_sources(&wisp_corpus::bundle_root(spec)).unwrap();
        let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
        let must = must_findings(&mut repo).unwrap();
        assert!(
            must.is_empty(),
            "bundle {} MUST findings: {:?}",
            spec.id,
            must
        );
        if spec.lossless_strip {
            let export = dir.path().join("export");
            assert_lossless_strip(&mut repo, &root, &before, &export)
                .unwrap_or_else(|e| panic!("bundle {} I3: {}", spec.id, e));
        }
        assert!(wisp_engine::query::fsck(&mut repo).is_ok(), "fsck {}", spec.id);
        assert!(
            wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok(),
            "hot_check {}",
            spec.id
        );
    }
}

#[test]
fn corpus_match_scenarios() {
    for spec in &catalog().scenario {
        let scenario = load_match_scenario(spec).unwrap();
        let parsed = parse_file(&scenario.input);
        let prior: Vec<PriorSection> = scenario
            .prior
            .iter()
            .map(|p| PriorSection {
                id: p.id.parse().unwrap(),
                level: p.level,
                heading: p.heading.clone(),
                heading_line: p.heading_line.clone(),
                body: p.body.clone(),
            })
            .collect();
        let ctx = ScenarioCtx::from_spec(&scenario.ctx);
        let r = match_sections(parsed.sections(), &prior, &ctx);
        for exp in &scenario.expect {
            let d = &r.decisions[exp.index];
            assert!(
                d.rule().contains(&exp.rule_contains),
                "scenario {} index {}: expected rule containing {:?}, got {:?} ({})",
                scenario.id,
                exp.index,
                exp.rule_contains,
                d,
                d.rule()
            );
        }
        if scenario.id == "merge-sections" {
            assert_eq!(r.removed.len(), 1);
            assert!(r.removed[0].merged_into.is_some());
        }
        if scenario.id == "duplicate-anchor" {
            assert!(matches!(r.decisions[2], Decision::Copied { .. }));
        }
    }
}

#[test]
fn corpus_policy_attested_computation_denies_process() {
    let cat = catalog();
    let spec = cat.bundle("policy-attested").expect("policy-attested bundle");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let path = "computations/sum";
    let concept = wisp_store::Hot::concept_by_path(hot.conn(), path)
        .unwrap()
        .expect("concept");
    let sections = wisp_store::Hot::sections_of(hot.conn(), concept.concept).unwrap();
    let comp = sections
        .iter()
        .find(|s| s.heading == "Computation")
        .expect("Computation section");
    let op = ApplyOp {
        op: "update_section".into(),
        path: path.into(),
        section: Some(comp.section),
        base_version: Some(comp.version.to_string()),
        heading: None,
        level: None,
        content: Some("tampered".into()),
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
    };
    let err = wisp_engine::headless::single(
        &mut repo,
        None,
        Some("process:corpus".into()),
        None,
        None,
        op,
    )
    .unwrap_err();
    assert_eq!(err.code, wisp_model::ExitCode::Refused);
}

#[test]
fn corpus_v01_migration_plan_nonempty() {
    let cat = catalog();
    let spec = cat.bundle("v01-migration").expect("v01-migration");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let plan = wisp_engine::query::migrate(&mut repo, MAIN_REF, "0.2", true).unwrap();
    let n = plan["steps"].as_u64().unwrap_or(0);
    assert!(n > 0, "v01 bundle should produce migration steps, got {plan}");
}

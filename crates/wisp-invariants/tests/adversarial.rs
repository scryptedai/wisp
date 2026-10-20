//! Adversarial integration tests: record refusal, identity repair, idempotence.

use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_invariants::{catalog, init_and_record_bundle};
use wisp_model::ExitCode;
use wisp_parse::parse_file;
use wisp_store::Hot;

fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

fn rec(repo: &mut Repository, actor: &str) -> wisp_engine::record::RecordResult {
    record(
        repo,
        &RecordOptions {
            actor: Some(actor.into()),
            message: Some("adversarial".into()),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn adversarial_record_is_idempotent_on_clean_tree() {
    let cat = catalog();
    let spec = cat.bundle("adversarial-suite").expect("adversarial-suite");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let r1 = rec(&mut repo, "human:corpus");
    assert_eq!(r1.ops, 0, "second record on clean tree: {r1:?}");
    let r2 = rec(&mut repo, "human:corpus");
    assert_eq!(r2.ops, 0);
}

#[test]
fn adversarial_editing_generated_index_refused() {
    let cat = catalog();
    let spec = cat.bundle("adversarial-suite").expect("adversarial-suite");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let index = root.join("index.md");
    let mut text = std::fs::read_to_string(&index).unwrap();
    text.push_str("\n<!-- tamper -->\n");
    std::fs::write(&index, text).unwrap();
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:corpus".into()),
            message: Some("tamper".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Refused);
}

#[test]
fn adversarial_strip_wisp_id_at_same_path_is_repair_not_fork() {
    let cat = catalog();
    let spec = cat.bundle("identity-gauntlet").expect("identity-gauntlet");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let path = "notes/chaos";
    let concept_before = Hot::concept_by_path(hot.conn(), path)
        .unwrap()
        .expect("concept");
    let secs = Hot::sections_of(hot.conn(), concept_before.concept).unwrap();
    let alpha_before = secs
        .iter()
        .find(|s| s.body.contains("alpha body"))
        .unwrap_or_else(|| {
            panic!(
                "alpha section; headings={:?}",
                secs.iter()
                    .map(|s| (&s.heading, s.level))
                    .collect::<Vec<_>>()
            )
        });
    let file = root.join("notes/chaos.md");
    let mut text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("wisp_id:"));
    text = text
        .lines()
        .filter(|l| !l.starts_with("wisp_id:"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&file, text).unwrap();
    let _r = rec(&mut repo, "human:corpus");
    let hot = repo.hot(MAIN_REF).unwrap();
    let concept_after = Hot::concept_by_path(hot.conn(), path)
        .unwrap()
        .expect("same concept");
    assert_eq!(concept_before.concept, concept_after.concept);
    let after_secs = Hot::sections_of(hot.conn(), concept_after.concept).unwrap();
    let alpha_after = after_secs
        .iter()
        .find(|s| s.body.contains("alpha body"))
        .expect("alpha after");
    assert_eq!(alpha_before.section, alpha_after.section, "I7 section id");
}

#[test]
fn adversarial_reorder_sections_preserves_ids() {
    let cat = catalog();
    let spec = cat.bundle("identity-gauntlet").expect("identity-gauntlet");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &unlock()).unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let concept = Hot::concept_by_path(hot.conn(), "notes/chaos")
        .unwrap()
        .expect("concept");
    let ids_before: Vec<_> = Hot::sections_of(hot.conn(), concept.concept)
        .unwrap()
        .into_iter()
        .filter(|s| s.level > 0)
        .map(|s| (s.heading.clone(), s.section))
        .collect();
    let file = root.join("notes/chaos.md");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines = text.lines().collect::<Vec<_>>();
    let beta_start = lines.iter().position(|l| l.contains("## Beta")).unwrap();
    let gamma_start = lines.iter().position(|l| l.contains("## Gamma")).unwrap();
    assert!(beta_start < gamma_start);
    let tail = lines.split_off(gamma_start);
    let mid = lines.split_off(beta_start);
    let mut new_lines = lines;
    new_lines.extend(tail);
    new_lines.extend(mid);
    std::fs::write(&file, new_lines.join("\n")).unwrap();
    let _ = rec(&mut repo, "human:corpus");
    let hot = repo.hot(MAIN_REF).unwrap();
    let ids_after: Vec<_> = Hot::sections_of(hot.conn(), concept.concept)
        .unwrap()
        .into_iter()
        .filter(|s| s.level > 0)
        .map(|s| (s.heading.clone(), s.section))
        .collect();
    for (heading, id) in &ids_before {
        let after = ids_after
            .iter()
            .find(|(h, _)| h == heading)
            .expect("heading");
        assert_eq!(*id, after.1, "section id for {heading}");
    }
}

#[test]
fn adversarial_stray_anchor_file_in_suite_warns() {
    let path = wisp_corpus::data_dir().join("bundles/adversarial-suite/anchors/stray-in-body.md");
    let text = std::fs::read_to_string(path).unwrap();
    let parsed = parse_file(&text);
    assert!(
        parsed
            .sectioned
            .warnings
            .iter()
            .any(|w| w.contains("anchor")),
        "expected stray-anchor warning, got {:?}",
        parsed.sectioned.warnings
    );
}

#[test]
fn adversarial_crlf_bundle_file_parses() {
    let path = wisp_corpus::data_dir().join("bundles/adversarial-suite/frontmatter/crlf-note.md");
    let bytes = std::fs::read(path).unwrap();
    assert!(
        bytes.windows(2).any(|w| w == b"\r\n"),
        "fixture must be CRLF"
    );
    let text = String::from_utf8(bytes).unwrap();
    let parsed = parse_file(&text);
    assert_eq!(parsed.line_ending, "\r\n");
    assert!(parsed.type_value().is_some());
}

#[test]
fn adversarial_bom_concept_starts_with_bom() {
    let cat = catalog();
    let spec = cat
        .concept
        .iter()
        .find(|c| c.id == "parse.bom-utf8")
        .unwrap();
    let bytes = std::fs::read(wisp_corpus::data_dir().join(&spec.path)).unwrap();
    assert_eq!(&bytes[0..3], &[0xEF, 0xBB, 0xBF]);
}

//! Harness for corpus-driven invariant tests. Integration tests live in `tests/`.

use std::path::Path;
use wisp_corpus::{BundleSpec, Catalog};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_model::Result;
use wisp_profile::Level;

pub fn catalog() -> Catalog {
    Catalog::load().expect("corpus catalog.toml must parse")
}

/// Copy bundle → `wisp init` → `record` with a fixed actor.
pub fn init_and_record_bundle(
    spec: &BundleSpec,
    dest: &Path,
    unlock: &Unlock,
) -> Result<Repository> {
    std::fs::create_dir_all(dest)?;
    wisp_corpus::copy_bundle(spec, dest).map_err(|e| {
        wisp_model::WispError::internal(format!("corpus copy: {e}"))
    })?;
    let mut repo = Repository::init(dest, &spec.profile, unlock)?;
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:corpus".into()),
            message: Some(format!("corpus: {}", spec.id)),
            ..Default::default()
        },
    )?;
    Ok(repo)
}

fn parse_findings(v: &serde_json::Value) -> Result<Vec<wisp_profile::Finding>> {
    serde_json::from_value(v["findings"].clone()).map_err(|e| {
        wisp_model::WispError::internal(format!("validate findings json: {e}"))
    })
}

/// MUST-level profile findings for the whole bundle at `MAIN_REF`.
pub fn must_findings(repo: &mut Repository) -> Result<Vec<wisp_profile::Finding>> {
    match wisp_engine::query::validate(repo, MAIN_REF, None, false) {
        Ok(v) => {
            let findings = parse_findings(&v)?;
            Ok(findings
                .into_iter()
                .filter(|f| f.level == Level::Must)
                .collect())
        }
        Err(e) => {
            let findings = parse_findings(&e.details)?;
            Ok(findings
                .into_iter()
                .filter(|f| f.level == Level::Must)
                .collect())
        }
    }
}

/// Checkout strip-anchors to `export_dir` and compare to pre-record snapshots (I3).
pub fn assert_lossless_strip(
    repo: &mut Repository,
    _bundle_root: &Path,
    before: &std::collections::HashMap<String, String>,
    export_dir: &Path,
) -> Result<()> {
    wisp_engine::checkout::checkout(repo, MAIN_REF, Some(export_dir), true, false)?;
    for (rel, orig) in before {
        let out_path = export_dir.join(rel);
        let out = std::fs::read_to_string(&out_path).map_err(|e| {
            wisp_model::WispError::internal(format!(
                "missing export {}: {e}",
                out_path.display()
            ))
        })?;
        let stripped = wisp_corpus::strip_managed_and_anchors(&out);
        let expect = wisp_corpus::strip_managed_and_anchors(orig);
        if stripped != expect {
            return Err(wisp_model::WispError::internal(format!(
                "I3 mismatch for {rel}\n--- expected ---\n{expect}\n--- got ---\n{stripped}"
            )));
        }
    }
    Ok(())
}

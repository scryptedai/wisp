//! Shared helpers for input-capture and `wisp impact` tests. Batches are built as JSON so the
//! tests exercise the public `apply` format, including fields added for impact tracing.

#![allow(dead_code)]

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use wisp_engine::headless::{apply, ApplyBatch};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_model::{ChangeId, ChangesetId, ConceptId, Result, SectionId};
use wisp_store::{Hot, Ledger};

pub const AGENT: &str = "claude-code/1";
pub const HUMAN: &str = "human:tim";

pub fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

/// `wisp init` + one human `record` of `files` (path relative to the bundle root, text).
pub fn fresh(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    std::fs::create_dir_all(&root).unwrap();
    for (rel, text) in files {
        write(&root, rel, text);
    }
    Repository::init(&root, "0.2", &unlock()).unwrap();
    let mut repo = Repository::open(&root, &unlock()).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some(HUMAN.into()),
            message: Some("initial".into()),
            ..Default::default()
        },
    )
    .unwrap();
    (dir, root, repo)
}

pub fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

pub fn concept_id(repo: &mut Repository, ref_name: &str, path: &str) -> ConceptId {
    let hot = repo.hot(ref_name).unwrap();
    let p = path.trim_end_matches(".md");
    Hot::concept_by_path(hot.conn(), p)
        .unwrap()
        .unwrap_or_else(|| panic!("no concept {path} on {ref_name}"))
        .concept
}

/// `(section, current version)` of the section titled `heading`.
pub fn section(
    repo: &mut Repository,
    ref_name: &str,
    path: &str,
    heading: &str,
) -> (SectionId, ChangeId) {
    let c = concept_id(repo, ref_name, path);
    let hot = repo.hot(ref_name).unwrap();
    let s = Hot::sections_of(hot.conn(), c)
        .unwrap()
        .into_iter()
        .find(|s| s.heading == heading)
        .unwrap_or_else(|| panic!("no section {heading:?} in {path}"));
    (s.section, s.version)
}

pub fn head(repo: &mut Repository, ref_name: &str) -> ChangesetId {
    repo.head(ref_name).unwrap().expect("ref has a head")
}

pub fn apply_json(repo: &mut Repository, batch: Value) -> Result<Value> {
    let b: ApplyBatch = serde_json::from_value(batch).expect("batch json");
    apply(repo, b, None)
}

/// One `update_section` against the section's current version.
#[allow(clippy::too_many_arguments)]
pub fn update(
    repo: &mut Repository,
    ref_name: &str,
    actor: &str,
    path: &str,
    heading: &str,
    content: &str,
    inputs: Value,
    extra: Value,
) -> Result<Value> {
    let (sid, ver) = section(repo, ref_name, path, heading);
    let mut op = json!({
        "op": "update_section", "path": path, "section": sid.to_string(),
        "base_version": ver.to_string(), "content": content
    });
    let mut batch = json!({
        "ref": ref_name, "actor": actor, "message": format!("update {path}#{heading}"),
        "ops": [], "inputs": inputs
    });
    if let Value::Object(m) = extra {
        for (k, v) in m {
            if let Some(field) = k.strip_prefix("op.") {
                op[field] = v;
            } else {
                batch[k] = v;
            }
        }
    }
    batch["ops"] = json!([op]);
    apply_json(repo, batch)
}

/// Input JSON for `path#heading`, optionally pinned at `at`.
pub fn input(
    repo: &mut Repository,
    ref_name: &str,
    path: &str,
    heading: Option<&str>,
    at: Option<ChangesetId>,
) -> Value {
    let mut v = json!({ "concept": path });
    if let Some(h) = heading {
        let (sid, _) = section(repo, ref_name, path, h);
        v["section"] = json!(sid.to_string());
    }
    if let Some(a) = at {
        v["at"] = json!(a.to_string());
    }
    v
}

/// The version (change id) a write produced for its first section change.
pub fn written_version(result: &Value) -> ChangeId {
    result["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("section").is_some())
        .and_then(|c| c["version"].as_str())
        .expect("a section change")
        .parse()
        .unwrap()
}

pub fn ledger_inputs(repo: &Repository, version: ChangeId) -> Vec<wisp_model::InputRef> {
    Ledger::get_change(repo.ledger.conn(), version)
        .unwrap()
        .expect("change exists")
        .inputs
}

pub fn main_ref() -> &'static str {
    MAIN_REF
}

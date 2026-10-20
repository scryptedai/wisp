//! Explicit `semantic` mode smoke (cases: `contrib/search-battery/cases.toml`).

#![cfg(feature = "semantic")]

use wisp_engine::query::search;
use wisp_engine::repo::{Unlock, MAIN_REF};
use wisp_invariants::{catalog, init_and_record_bundle};

#[test]
fn search_battery_semantic_mode_smoke() {
    let cat = catalog();
    let spec = cat.bundle("search-battery").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut repo =
        init_and_record_bundle(spec, &dir.path().join("bundle"), &Unlock::default()).unwrap();
    let v = search(
        &mut repo,
        MAIN_REF,
        "RAM depleted process terminated",
        "semantic",
        5,
    )
    .expect("semantic search");
    let results = v["results"].as_array().cloned().unwrap_or_default();
    assert!(
        results
            .first()
            .is_some_and(|h| h["heading"].as_str().unwrap_or("").contains("sec-oom")),
        "semantic mode should rank OOM section first"
    );
}

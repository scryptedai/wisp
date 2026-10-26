//! `--local-only` must block explicit semantic search.

use wisp_engine::query::search_with_params;
use wisp_engine::repo::{Unlock, MAIN_REF};
use wisp_engine::search::SearchParams;
use wisp_invariants::{catalog, init_and_record_bundle};

#[test]
fn local_only_rejects_semantic_mode() {
    let cat = catalog();
    let spec = cat.bundle("search-battery").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut repo =
        init_and_record_bundle(spec, &dir.path().join("bundle"), &Unlock::default()).unwrap();
    let params = SearchParams {
        threshold: None,
        local_only: true,
        field: None,
    };
    let err = search_with_params(&mut repo, MAIN_REF, "RAM depleted", "semantic", 5, &params)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("local-only"),
        "expected local-only error, got: {err}"
    );
}

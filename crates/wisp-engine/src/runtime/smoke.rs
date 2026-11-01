//! Smoke test runtime RPC wiring.

use crate::manifest::RuntimeSection;
use crate::repo::{Repository, Unlock, MAIN_REF};
use crate::runtime::{self, RuntimeServer};

fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

#[test]
fn runtime_ping_and_apply_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("WISP_RUNTIME_DIRECT", "1");
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    let mut m = repo.manifest.clone();
    m.runtime.enabled = true;
    m.save(&repo.wisp_dir.join("manifest.toml")).unwrap();
    drop(repo);
    std::env::remove_var("WISP_RUNTIME_DIRECT");
    let cfg = RuntimeSection {
        enabled: true,
        timeout_ms: 120_000,
        ..Default::default()
    };
    let _rt = RuntimeServer::spawn(root.to_path_buf(), unlock(), cfg).expect("spawn");
    let wisp = root.join(".wisp");
    runtime::rpc_ping(&wisp, 5_000).expect("ping");
    let batch = serde_json::json!({
        "ref": MAIN_REF,
        "actor": "test/runtime",
        "message": "smoke",
        "ops": [{
            "op": "create_concept",
            "path": "notes/smoke.md",
            "type": "note",
            "content": "# smoke\n"
        }],
        "inputs": [],
        "inputs_complete": false
    });
    let out =
        runtime::rpc_apply(&wisp, "test/runtime", batch, Some(MAIN_REF), 30_000).expect("apply");
    assert!(out.get("head").is_some());
    let pack = runtime::rpc_pack(
        &wisp,
        "test/runtime",
        MAIN_REF,
        &["notes/smoke.md".into()],
        None,
        None,
        true,
        5_000,
    )
    .expect("pack");
    assert_eq!(pack["fresh"].as_bool(), Some(true));
}

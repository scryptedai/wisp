use std::path::PathBuf;
use wisp_engine::manifest::SubscriberSection;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock};
use wisp_feed::run_external_subscribers;
use wisp_store::Ledger;

fn mock_subscriber_cmd() -> Vec<String> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/mock_ack_subscriber.py");
    vec!["python3".into(), script.to_string_lossy().into_owned()]
}

#[test]
fn external_subscriber_advances_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    std::fs::create_dir_all(&root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    Repository::init(&root, "0.2", &Unlock::default()).unwrap();
    let orders = "---\ntype: Doc\ntitle: T\n---\n\n## A\n\none\n";
    std::fs::write(root.join("a.md"), orders).unwrap();
    let mut repo = Repository::open(&root, &Unlock::default()).unwrap();
    repo.manifest.subscribers.push(SubscriberSection {
        name: "mock".into(),
        builtin: None,
        command: Some(mock_subscriber_cmd()),
    });
    repo.manifest
        .save(&repo.wisp_dir.join("manifest.toml"))
        .unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:alex".into()),
            message: Some("test".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let max = Ledger::max_seq(repo.ledger.conn()).unwrap();
    let pending = wisp_engine::query::feed(&repo, 0).unwrap().len();
    assert_eq!(Ledger::cursor(repo.ledger.conn(), "mock").unwrap(), 0);
    let out = run_external_subscribers(&mut repo).unwrap();
    let ran = out["subscribers"].as_array().unwrap();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0]["delivered"].as_u64().unwrap() as usize, pending);
    assert_eq!(Ledger::cursor(repo.ledger.conn(), "mock").unwrap(), max);
    let out2 = run_external_subscribers(&mut repo).unwrap();
    assert_eq!(out2["subscribers"][0]["delivered"], 0);
}

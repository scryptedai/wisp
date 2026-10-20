//! Trusted local-path replica sync (§11.6): push then pull yields the same ref head.

use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_store::Ledger;
use wisp_sync::{init_trusted_replica, pull, push};

#[test]
fn sync_push_pull_same_head() {
    let dir = tempfile::tempdir().unwrap();
    let a_root = dir.path().join("a");
    let b_root = dir.path().join("b");
    std::fs::create_dir_all(&a_root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    std::fs::write(
        a_root.join("note.md"),
        "---\ntype: Note\ntitle: Roundtrip\n---\n\n## Body\n\ntext\n",
    )
    .unwrap();

    let mut a = Repository::init(&a_root, "0.2", &Unlock::default()).unwrap();
    record(
        &mut a,
        &RecordOptions {
            actor: Some("human:sync".into()),
            message: Some("init".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let head0 = Ledger::head(a.ledger.conn(), MAIN_REF).unwrap().unwrap();

    let mut b = init_trusted_replica(&b_root, &a, &Unlock::default()).unwrap();
    push(&mut a, &b_root, MAIN_REF, &Unlock::default()).unwrap();
    assert_eq!(
        Ledger::head(b.ledger.conn(), MAIN_REF).unwrap(),
        Some(head0)
    );

    std::fs::write(
        a_root.join("note.md"),
        "---\ntype: Note\ntitle: Roundtrip\n---\n\n## Body\n\ntext v2\n",
    )
    .unwrap();
    record(
        &mut a,
        &RecordOptions {
            actor: Some("human:sync".into()),
            message: Some("update".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let head1 = Ledger::head(a.ledger.conn(), MAIN_REF).unwrap().unwrap();

    pull(&mut b, &a_root, MAIN_REF, &Unlock::default()).unwrap();
    assert_eq!(Ledger::head(b.ledger.conn(), MAIN_REF).unwrap(), Some(head1));
}

//! Trusted replica sync: erasures and scope keys follow the source (§11.6 deletions win).

use wisp_engine::erase::{erase, shred, EraseTarget};
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_model::ChangeId;
use wisp_model::Reason;
use wisp_store::{Hot, Ledger};
use wisp_sync::{init_trusted_replica, pull, push};

fn init_pair() -> (tempfile::TempDir, Repository, Repository) {
    let dir = tempfile::tempdir().unwrap();
    let a_root = dir.path().join("a");
    let b_root = dir.path().join("b");
    std::fs::create_dir_all(&a_root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    std::fs::write(
        a_root.join("note.md"),
        "---\ntype: Note\ntitle: Sync erase\n---\n\n## Body\n\nsecret\n",
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
    drop(a);
    let b = init_trusted_replica(&b_root, &a_root, &Unlock::default()).unwrap();
    let mut a = Repository::open(&a_root, &Unlock::default()).unwrap();
    push(&mut a, &b_root, MAIN_REF, &Unlock::default()).unwrap();
    (dir, a, b)
}

#[test]
fn sync_pull_after_erase_drops_scope_keys() {
    let (_dir, mut a, mut b) = init_pair();
    let a_root = a.root.clone();

    let conn = a.hot(MAIN_REF).unwrap().conn();
    let concept = Hot::concept_by_path(conn, "note").unwrap().unwrap();
    let section = Hot::sections_of(conn, concept.concept).unwrap()[0].section;
    let scope = Ledger::scope_of_section(a.ledger.conn(), section)
        .unwrap()
        .unwrap()
        .id;
    assert!(a.keystore.has(scope).unwrap());
    assert!(b.keystore.has(scope).unwrap());

    erase(
        &mut a,
        MAIN_REF,
        &EraseTarget::Path(format!("note#{section}")),
        Reason::Policy,
        true,
        "human:sync",
    )
    .unwrap();
    assert!(!a.keystore.has(scope).unwrap());

    pull(&mut b, &a_root, MAIN_REF, &Unlock::default()).unwrap();
    assert!(!b.keystore.has(scope).unwrap());
    let row = Ledger::scope(b.ledger.conn(), scope).unwrap().unwrap();
    assert!(row.erased);
    assert!(row.shredded);
    assert!(!Ledger::tombstones(b.ledger.conn()).unwrap().is_empty());
    assert!(Hot::section(b.hot(MAIN_REF).unwrap().conn(), section)
        .unwrap()
        .is_none());
}

#[test]
fn sync_pull_does_not_restore_keys_for_locally_shredded_scopes() {
    let (_dir, mut a, mut b) = init_pair();
    let a_root = a.root.clone();

    let conn = a.hot(MAIN_REF).unwrap().conn();
    let concept = Hot::concept_by_path(conn, "note").unwrap().unwrap();
    let section = Hot::sections_of(conn, concept.concept).unwrap()[0].section;
    let scope = Ledger::scope_of_section(a.ledger.conn(), section)
        .unwrap()
        .unwrap()
        .id;

    Ledger::mark_scope_erased(b.ledger.conn(), scope).unwrap();
    Ledger::set_scope_payloads_absent(b.ledger.conn(), scope).unwrap();
    shred(&mut b, &[scope], ChangeId::new(), &[]).unwrap();
    assert!(!b.keystore.has(scope).unwrap());
    assert!(
        Ledger::scope(b.ledger.conn(), scope)
            .unwrap()
            .unwrap()
            .shredded
    );
    assert!(a.keystore.has(scope).unwrap());
    assert_eq!(
        Ledger::head(a.ledger.conn(), MAIN_REF).unwrap(),
        Ledger::head(b.ledger.conn(), MAIN_REF).unwrap()
    );

    pull(&mut b, &a_root, MAIN_REF, &Unlock::default()).unwrap();
    assert!(!b.keystore.has(scope).unwrap());
}

#[test]
fn sync_pull_copies_scope_keys_for_new_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let a_root = dir.path().join("a");
    let b_root = dir.path().join("b");
    std::fs::create_dir_all(&a_root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    std::fs::write(
        a_root.join("one.md"),
        "---\ntype: Note\ntitle: One\n---\n\n## A\n\nfirst\n",
    )
    .unwrap();

    let mut a = Repository::init(&a_root, "0.2", &Unlock::default()).unwrap();
    record(
        &mut a,
        &RecordOptions {
            actor: Some("human:sync".into()),
            message: Some("one".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(a);

    let mut b = init_trusted_replica(&b_root, &a_root, &Unlock::default()).unwrap();
    let mut a = Repository::open(&a_root, &Unlock::default()).unwrap();
    push(&mut a, &b_root, MAIN_REF, &Unlock::default()).unwrap();

    std::fs::write(
        a_root.join("two.md"),
        "---\ntype: Note\ntitle: Two\n---\n\n## B\n\nsecond\n",
    )
    .unwrap();
    record(
        &mut a,
        &RecordOptions {
            actor: Some("human:sync".into()),
            message: Some("two".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let conn = a.hot(MAIN_REF).unwrap().conn();
    let two = Hot::concept_by_path(conn, "two").unwrap().unwrap();
    let two_section = Hot::sections_of(conn, two.concept).unwrap()[0].section;
    let two_scope = Ledger::scope_of_section(a.ledger.conn(), two_section)
        .unwrap()
        .unwrap()
        .id;
    assert!(a.keystore.has(two_scope).unwrap());
    assert!(!b.keystore.has(two_scope).unwrap());

    let out = pull(&mut b, &a_root, MAIN_REF, &Unlock::default()).unwrap();
    assert!(out["scope_keys"].as_u64().unwrap() >= 1);
    assert!(b.keystore.has(two_scope).unwrap());
    assert_eq!(
        Ledger::head(a.ledger.conn(), MAIN_REF).unwrap(),
        Ledger::head(b.ledger.conn(), MAIN_REF).unwrap()
    );
}

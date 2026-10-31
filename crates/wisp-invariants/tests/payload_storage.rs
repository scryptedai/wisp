//! Default plain cold storage vs optional encrypted init.

use std::path::Path;
use wisp_engine::query;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::{InitOptions, PayloadStorage, Repository, Unlock, MAIN_REF};

fn unlock() -> Unlock {
    std::env::remove_var("WISP_MASTER_KEY");
    Unlock::default()
}

#[test]
fn default_init_is_plain_without_master_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::init(root, "0.2", &unlock()).unwrap();
    assert_eq!(repo.payload_storage(), PayloadStorage::Plain);
    assert!(!root.join(".wisp/keys/master.key").exists());
    let manifest =
        wisp_engine::manifest::Manifest::load(&root.join(".wisp/manifest.toml")).unwrap();
    assert_eq!(
        PayloadStorage::from_manifest_field(manifest.repo.storage),
        PayloadStorage::Plain
    );
}

#[test]
fn plain_repo_record_and_fsck() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("note.md"),
        "---\ntype: Note\n---\n## Body\n\nhello plain\n",
    )
    .unwrap();
    let mut repo = Repository::init(root, "0.2", &unlock()).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:test".into()),
            message: Some("plain storage".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(repo);
    let mut repo = Repository::open(root, &unlock()).unwrap();
    query::fsck(&mut repo).expect("fsck plain repo");
    let payload_dir = root.join(".wisp/payloads");
    let mut saw_plain = false;
    walk_payloads(&payload_dir, &mut |bytes| {
        if bytes.len() >= 6 && &bytes[..4] == b"WSP1" && bytes[4] == 2 {
            saw_plain = true;
        }
    });
    assert!(saw_plain, "expected WSP1 v2 plain payload on disk");
}

#[test]
fn encrypt_init_uses_ciphertext_and_master_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("secret.md"),
        "---\ntype: Note\n---\n## Body\n\nclassified\n",
    )
    .unwrap();
    let mut repo =
        Repository::init_with(root, "0.2", &unlock(), InitOptions { encrypt: true }).unwrap();
    assert_eq!(repo.payload_storage(), PayloadStorage::Encrypted);
    assert!(root.join(".wisp/keys/master.key").exists());
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:test".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(repo);
    let mut repo = Repository::open(root, &unlock()).unwrap();
    query::fsck(&mut repo).expect("fsck encrypted repo");
    let mut saw_encrypted = false;
    walk_payloads(&root.join(".wisp/payloads"), &mut |bytes| {
        if bytes.len() >= 30 && &bytes[..4] == b"WSP1" && bytes[4] == 1 {
            saw_encrypted = true;
        }
    });
    assert!(saw_encrypted, "expected WSP1 v1 encrypted payload on disk");
}

#[test]
fn manifest_without_storage_field_is_encrypted_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    Repository::init_with(root, "0.2", &unlock(), InitOptions { encrypt: true }).unwrap();
    let path = root.join(".wisp/manifest.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text = text
        .lines()
        .filter(|l| !l.starts_with("storage"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, text).unwrap();
    let manifest = wisp_engine::manifest::Manifest::load(&path).unwrap();
    assert_eq!(manifest.repo.storage, None);
    assert_eq!(
        PayloadStorage::from_manifest_field(manifest.repo.storage),
        PayloadStorage::Encrypted
    );
    let repo = Repository::open(root, &unlock()).unwrap();
    assert_eq!(repo.payload_storage(), PayloadStorage::Encrypted);
    use wisp_store::Ledger;
    assert!(Ledger::head(repo.ledger.conn(), MAIN_REF)
        .unwrap()
        .is_none());
}

fn walk_payloads(dir: &Path, f: &mut dyn FnMut(&[u8])) {
    if !dir.is_dir() {
        return;
    }
    for ent in std::fs::read_dir(dir).unwrap().flatten() {
        let p = ent.path();
        if p.is_dir() {
            walk_payloads(&p, f);
        } else if let Ok(bytes) = std::fs::read(&p) {
            f(&bytes);
        }
    }
}

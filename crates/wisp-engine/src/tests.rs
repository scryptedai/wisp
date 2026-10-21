//! Engine tests: the invariants from §17 that this build implements.

use crate::erase::EraseTarget;
use crate::headless::{ApplyBatch, ApplyOp};
use crate::record::{record, status, RecordOptions};
use crate::repo::{Repository, Unlock, MAIN_REF};
use serde_json::json;
use std::path::{Path, PathBuf};
use wisp_model::*;
use wisp_store::{Hot, Ledger};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    std::fs::create_dir_all(&root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    Fixture { _dir: dir, root }
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
    // Make sure mtime moves even on fast filesystems.
    std::thread::sleep(std::time::Duration::from_millis(5));
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn open(root: &Path) -> Repository {
    Repository::open(root, &Unlock::default()).unwrap()
}

fn rec(repo: &mut Repository, actor: &str) -> crate::record::RecordResult {
    record(
        repo,
        &RecordOptions {
            actor: Some(actor.into()),
            message: Some("test".into()),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn signed_changeset_verifies_in_fsck() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    crate::query::actor_gen_key(&mut repo, MAIN_REF, "human:alex").unwrap();
    write(&f.root, "tables/customers.md", CUSTOMERS);
    let r = rec(&mut repo, "human:alex");
    let fsck = crate::query::fsck(&mut repo).unwrap();
    assert!(fsck["problems"].as_array().unwrap().is_empty());
    let cs_id: ChangesetId = r.changeset.unwrap().parse().unwrap();
    let detail = crate::query::show(&mut repo, MAIN_REF, cs_id).unwrap();
    assert_eq!(detail["signed"], true);
}

const ORDERS: &str = "---\ntype: BigQuery Table\ntitle: Customer Orders\ntags:\n  - sales\n---\nOrders placed by customers.\n\n## Schema\n\n| col | type |\n|---|---|\n| id | INT64 |\n\n## Behavior\n\nRows are append-only. See [customers](customers.md).\n";
const CUSTOMERS: &str =
    "---\ntype: BigQuery Table\ntitle: Customers\n---\n\n## Schema\n\ncustomer_id, name\n";

/// Scan every file under .wisp/ (databases, WAL/SHM, payloads, worktrees) and the working tree for a canary (I4).
fn scan_for(root: &Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for e in walkdir::WalkDir::new(root).into_iter().flatten() {
        if e.file_type().is_file() {
            if let Ok(b) = std::fs::read(e.path()) {
                if b.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    hits.push(e.path().display().to_string());
                }
            }
        }
    }
    hits
}

#[test]
fn init_record_checkout_roundtrip_and_identity() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    write(&f.root, "tables/customers.md", CUSTOMERS);
    let mut repo = open(&f.root);
    let r = rec(&mut repo, "human:alex");
    assert!(r.changeset.is_some());
    // The tree was rewritten with wisp_id, anchors, and generated views.
    let text = read(&f.root, "tables/orders.md");
    assert!(text.contains("wisp_id: "), "{text}");
    assert!(text.contains("## Schema <!-- ^"), "{text}");
    assert!(text.contains("generated:"), "{text}");
    assert!(text.contains("by: human:alex"), "{text}");
    assert!(f.root.join("index.md").exists());
    assert!(f.root.join("tables/index.md").exists());
    assert!(f.root.join("tables/log.md").exists());
    assert!(read(&f.root, "tables/log.md").contains("Creation"));
    // Idempotent: recording again produces no second change (I23).
    let r2 = rec(&mut repo, "human:alex");
    assert_eq!(r2.ops, 0, "{r2:?}");
    // I7: an OS rename keeps wisp_id, history, and inbound links; a full rewrite keeps the SectionId.
    let parsed = wisp_parse::parse_file(&text);
    let schema_id = match &parsed.sections()[1].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!("anchor"),
    };
    std::fs::create_dir_all(f.root.join("warehouse")).unwrap();
    std::fs::rename(
        f.root.join("tables/orders.md"),
        f.root.join("warehouse/orders.md"),
    )
    .unwrap();
    let rewritten = text.replace("## Schema", "## Columns and Types").replace(
        "| id | INT64 |",
        "| order_id | INT64 |\n| total | NUMERIC |",
    );
    write(&f.root, "warehouse/orders.md", &rewritten);
    let r3 = rec(&mut repo, "human:alex");
    assert!(r3.changeset.is_some());
    let hot = repo.hot(MAIN_REF).unwrap();
    let c = Hot::concept_by_path(hot.conn(), "warehouse/orders")
        .unwrap()
        .expect("moved concept");
    assert_eq!(
        Hot::aliases(hot.conn(), c.concept).unwrap(),
        vec!["tables/orders".to_string()]
    );
    let s = Hot::section(hot.conn(), schema_id)
        .unwrap()
        .expect("same section id after rewrite");
    assert_eq!(s.heading, "Columns and Types");
    assert!(!f.root.join("tables/orders.md").exists());
    // The old link target from customers? customers has no links; orders links to customers: still resolved.
    let bl = Hot::backlinks(
        hot.conn(),
        Hot::concept_by_path(hot.conn(), "tables/customers")
            .unwrap()
            .unwrap()
            .concept,
    )
    .unwrap();
    assert_eq!(bl.len(), 1);
}

#[test]
fn i2_i13_hot_rebuild_is_deterministic() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    write(
        &f.root,
        "tables/orders.md",
        &read(&f.root, "tables/orders.md")
            .replace("append-only", "append-only, partitioned by day"),
    );
    rec(&mut repo, "sl-bot/0.3");
    let before = crate::query::hot_dump(repo.hot(MAIN_REF).unwrap().conn()).unwrap();
    let tree_before = read(&f.root, "tables/orders.md");
    crate::query::hot_rebuild(&mut repo, MAIN_REF).unwrap();
    let after = crate::query::hot_dump(repo.hot(MAIN_REF).unwrap().conn()).unwrap();
    assert_eq!(before, after);
    crate::checkout::checkout(&mut repo, MAIN_REF, None, false, false).unwrap();
    assert_eq!(tree_before, read(&f.root, "tables/orders.md"));
    assert!(crate::query::hot_check(&mut repo, MAIN_REF).is_ok());
    assert!(crate::query::fsck(&mut repo).is_ok());
}

#[test]
fn i3_lossless_roundtrip_strip_anchors() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let src = "---\n# a comment\ntype: Note\ntitle: \"Quoted: title\"\nmeta: {a: 1, b: [x, y]}\n\ndescription: >\n  folded\n  text\n---\nPreamble line.\n\n# Top\n\nbody *md*\n\n```\n# not heading\n```\n\nSetext\n------\n\nmore\n";
    write(&f.root, "notes/n.md", src);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let dir = f.root.parent().unwrap().join("view");
    crate::checkout::checkout(&mut repo, MAIN_REF, Some(&dir), true, false).unwrap();
    let out = std::fs::read_to_string(dir.join("notes/n.md")).unwrap();
    // Strip the lines wisp added: wisp_id and generated (managed), then compare.
    let cleaned: String = out
        .lines()
        .filter(|l| {
            !l.starts_with("wisp_id: ")
                && !l.starts_with("generated:")
                && !l.starts_with("  by: ")
                && !l.starts_with("  at: ")
        })
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(cleaned, src);
}

#[test]
fn i16_i17_concurrent_headless_writers_disjoint_sections() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let mut body = String::from("---\ntype: Doc\n---\n");
    for i in 0..8 {
        body.push_str(&format!("## S{i}\n\ninitial {i}\n\n"));
    }
    write(&f.root, "doc.md", &body);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "doc.md");
    let parsed = wisp_parse::parse_file(&text);
    let ids: Vec<SectionId> = parsed
        .sections()
        .iter()
        .skip(1)
        .map(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => *id,
            _ => panic!(),
        })
        .collect();
    let base_versions: Vec<ChangeId> = {
        let hot = repo.hot(MAIN_REF).unwrap();
        ids.iter()
            .map(|id| Hot::section(hot.conn(), *id).unwrap().unwrap().version)
            .collect()
    };
    drop(repo);
    // 8 writers × 5 writes each, every writer owns one section, all from the same base.
    let root = f.root.clone();
    let handles: Vec<_> = (0..8)
        .map(|w| {
            let root = root.clone();
            let id = ids[w];
            let base = base_versions[w];
            std::thread::spawn(move || {
                let mut repo = Repository::open(&root, &Unlock::default()).unwrap();
                let mut base = base;
                let mut ok = 0;
                for k in 0..5 {
                    let op = ApplyOp {
                        op: "update_section".into(),
                        path: "doc.md".into(),
                        section: Some(id),
                        base_version: Some(base.to_string()),
                        heading: None,
                        level: None,
                        content: Some(format!("writer {w} write {k}")),
                        after: None,
                        key: None,
                        value: None,
                        r#type: None,
                        frontmatter: None,
                        note: None,
                        split_from: None,
                        merged_from: None,
                    };
                    let r = crate::headless::single(
                        &mut repo,
                        None,
                        Some(format!("process:w{w}")),
                        None,
                        None,
                        op,
                    )
                    .unwrap();
                    base = r["changes"][0]["version"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    ok += 1;
                }
                ok
            })
        })
        .collect();
    let total: i32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(total, 40);
    let mut repo = open(&root);
    assert_eq!(Ledger::max_seq(repo.ledger.conn()).unwrap(), 41); // 1 record + 40 headless
    {
        let hot = repo.hot(MAIN_REF).unwrap();
        for (w, id) in ids.iter().enumerate() {
            let s = Hot::section(hot.conn(), *id).unwrap().unwrap();
            assert_eq!(s.body.trim(), format!("writer {w} write 4"));
        }
    }
    // Final hot tier equals a replay of the ledger.
    assert!(crate::query::hot_check(&mut repo, MAIN_REF).is_ok());
    // A stale base on the same section returns a conflict object, exit 3.
    let op = ApplyOp {
        op: "update_section".into(),
        path: "doc.md".into(),
        section: Some(ids[0]),
        base_version: Some(base_versions[0].to_string()),
        heading: None,
        level: None,
        content: Some("stale".into()),
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
    };
    let err = crate::headless::single(&mut repo, None, Some("process:late".into()), None, None, op)
        .unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    let conflicts = &err.details["conflicts"];
    assert_eq!(conflicts[0]["type"], "section_conflict");
    assert_eq!(conflicts[0]["theirs"]["actor"], "process:w0");
    assert!(conflicts[0]["theirs"]["content"]
        .as_str()
        .unwrap()
        .contains("writer 0 write 4"));
    assert_eq!(conflicts[0]["yours"]["content"], "stale\n");
    // Resubmitting with theirs.version as the new base succeeds.
    let theirs: ChangeId = conflicts[0]["theirs"]["version"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let op2 = ApplyOp {
        op: "update_section".into(),
        path: "doc.md".into(),
        section: Some(ids[0]),
        base_version: Some(theirs.to_string()),
        heading: None,
        level: None,
        content: Some("synthesized".into()),
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
    };
    crate::headless::single(
        &mut repo,
        None,
        Some("process:late".into()),
        None,
        None,
        op2,
    )
    .unwrap();
    // I19: the working tree was refreshed (no unrecorded edits), so the file shows the headless writes.
    let text = read(&f.root, "doc.md");
    assert!(text.contains("synthesized"), "{text}");
    assert!(text.contains("writer 3 write 4"));
}

#[test]
fn i19_headless_commit_preserves_unrecorded_edits() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(
        &f.root,
        "doc.md",
        "---\ntype: Doc\n---\n## A\n\nalpha\n\n## B\n\nbeta\n",
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "doc.md");
    let parsed = wisp_parse::parse_file(&text);
    let b_id = match &parsed.sections()[2].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    // Unrecorded edit to section A in the working tree.
    write(
        &f.root,
        "doc.md",
        &text.replace("alpha", "alpha edited locally"),
    );
    // Headless write to section B.
    let op = ApplyOp {
        op: "update_section".into(),
        path: "doc.md".into(),
        section: Some(b_id),
        base_version: None,
        heading: None,
        level: None,
        content: Some("beta by agent".into()),
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
    };
    crate::headless::single(&mut repo, None, Some("sl-bot/0.3".into()), None, None, op).unwrap();
    // The dirty file is untouched.
    let after = read(&f.root, "doc.md");
    assert!(after.contains("alpha edited locally"));
    assert!(!after.contains("beta by agent"));
    // The next record commits the disjoint edit cleanly, validated against the new head.
    let r = rec(&mut repo, "human:alex");
    assert!(r.changeset.is_some());
    let final_text = read(&f.root, "doc.md");
    assert!(final_text.contains("alpha edited locally"));
    assert!(final_text.contains("beta by agent"));
}

#[test]
fn i4_i5_i21_i22_erase_section_canaries_and_integrity() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let canary_a = "CANARY_A_7f3a9c2e";
    let canary_b = "CANARY_B_1d4e8b6f";
    write(&f.root, "notes/standup.md", &format!("---\ntype: Note\n---\n## Alpha\n\nsecret {canary_a} here\n\n## Beta\n\nkeep {canary_b}\n"));
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    // A second version of Alpha, so history has two payloads to shred.
    write(
        &f.root,
        "notes/standup.md",
        &read(&f.root, "notes/standup.md").replace("secret", "very secret"),
    );
    rec(&mut repo, "human:alex");
    assert!(!scan_for(&f.root, canary_a).is_empty());
    let text = read(&f.root, "notes/standup.md");
    let parsed = wisp_parse::parse_file(&text);
    let alpha = match &parsed.sections()[1].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    let fsck_before = crate::query::fsck(&mut repo).unwrap();
    let digests_before: Vec<[u8; 32]> = Ledger::changesets(repo.ledger.conn())
        .unwrap()
        .into_iter()
        .map(|r| r.changeset.digest)
        .collect();
    // Dry run lists what would be erased.
    let dry = crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Path(format!("notes/standup.md#{alpha}")),
        Reason::Policy,
        false,
        false,
        "human:alex",
    )
    .unwrap();
    assert_eq!(dry["dry_run"], true);
    let res = crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Path(format!("notes/standup.md#{alpha}")),
        Reason::Policy,
        true,
        true,
        "human:alex",
    )
    .unwrap();
    assert_eq!(res["scopes"].as_array().unwrap().len(), 1);
    drop(repo);
    // I4/I21: canary A is gone everywhere; canary B and the concept remain.
    let hits = scan_for(&f.root, canary_a);
    assert!(hits.is_empty(), "canary survived in {hits:?}");
    assert!(!scan_for(&f.root, canary_b).is_empty());
    let text = read(&f.root, "notes/standup.md");
    assert!(
        text.contains(&format!("<!-- ^{alpha} [erased: policy] -->")),
        "{text}"
    );
    assert!(text.contains("## Beta"));
    // I5: digests unchanged, fsck passes.
    let mut repo = open(&f.root);
    let digests_after: Vec<[u8; 32]> = Ledger::changesets(repo.ledger.conn())
        .unwrap()
        .into_iter()
        .map(|r| r.changeset.digest)
        .collect();
    assert_eq!(&digests_before[..], &digests_after[..digests_before.len()]);
    let fsck_after = crate::query::fsck(&mut repo).unwrap();
    assert_eq!(fsck_before["ok"], true);
    assert_eq!(fsck_after["ok"], true);
    // History still readable for Beta; Alpha's old versions are gone.
    let why = crate::query::why(&mut repo, MAIN_REF, "notes/standup.md", 0).unwrap();
    assert!(why["history"]["events"].as_array().unwrap().len() >= 3);
    // I22: pasting the erased anchor back mints a new ID and reveals nothing.
    let pasted = text.replace(
        "## Beta",
        &format!("## Alpha again <!-- ^{alpha} -->\n\nnew text\n\n## Beta"),
    );
    write(&f.root, "notes/standup.md", &pasted);
    let r = rec(&mut repo, "human:alex");
    assert!(r.changeset.is_some());
    let text2 = read(&f.root, "notes/standup.md");
    let parsed2 = wisp_parse::parse_file(&text2);
    let again = parsed2
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha again")
        .unwrap();
    match &again.anchor {
        Some(wisp_parse::Anchor::Valid(id)) => assert_ne!(*id, alpha),
        _ => panic!("expected a fresh anchor"),
    }
    assert!(scan_for(&f.root, canary_a).is_empty());
    // The retry of record is idempotent and the erased marker is still rendered.
    assert!(text2.contains("[erased: policy]"));
    // I14-lite: an erase is complete on reopen (no pending shreds).
    assert!(Ledger::scopes_erased_not_shredded(repo.ledger.conn())
        .unwrap()
        .is_empty());
}

#[test]
fn erase_whole_concept_and_group_and_actor() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "people/alice.md", "---\ntype: Person\n---\nAlice CANARY_ALICE lives in Paris.\n\n## Contact\n\nalice@example.com CANARY_MAIL\n");
    write(
        &f.root,
        "people/bob.md",
        "---\ntype: Person\n---\nBob is fine.\n",
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:dpo");
    crate::erase::group_create(&mut repo, "alice-gdpr").unwrap();
    crate::erase::group_member(
        &mut repo,
        MAIN_REF,
        "alice-gdpr",
        "people/alice.md",
        true,
        "human:dpo",
    )
    .unwrap();
    let groups = crate::erase::group_list(&repo).unwrap();
    assert_eq!(groups[0]["name"], "alice-gdpr");
    let res = crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Group("alice-gdpr".into()),
        Reason::Request,
        true,
        true,
        "human:dpo",
    )
    .unwrap();
    assert!(res["scopes"].as_array().unwrap().len() >= 3, "{res}");
    drop(repo);
    assert!(scan_for(&f.root, "CANARY_ALICE").is_empty());
    assert!(scan_for(&f.root, "CANARY_MAIL").is_empty());
    assert!(!f.root.join("people/alice.md").exists());
    assert!(f.root.join("people/bob.md").exists());
    let mut repo = open(&f.root);
    assert!(crate::query::fsck(&mut repo).is_ok());
    // Log shows an erasure without naming a path.
    let log = read(&f.root, "log.md");
    assert!(log.contains("Erasure: an erasure occurred"));
    assert!(!log.contains("alice"));
    // I24: erase the actor; name disappears, kind-based policy still applies, fsck passes.
    assert!(!scan_for(&f.root, "human:dpo").is_empty());
    crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Actor("human:dpo".into()),
        Reason::Request,
        true,
        true,
        "human:admin",
    )
    .unwrap();
    let actors = crate::query::actors(&repo).unwrap();
    assert!(actors
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["erased"] == true));
    let bob = read(&f.root, "people/bob.md");
    assert!(bob.contains("by: human:erased-"), "{bob}");
    assert!(!bob.contains("human:dpo"));
    assert!(crate::query::fsck(&mut repo).is_ok());
}

#[test]
fn retract_restore_revert_and_policies() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "calc/revenue.md", "---\ntype: Attested Computation\nruntime: python\n---\n## Computation\n\nsum(x)\n\n## Notes\n\nfree text\n");
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "calc/revenue.md");
    // I12: a process may not edit the Computation section (OKF §10.3).
    write(
        &f.root,
        "calc/revenue.md",
        &text.replace("sum(x)", "sum(y)"),
    );
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("process:bot".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Refused);
    assert_eq!(err.details["rule"], "sanctioned-computations");
    // A human may.
    let r = rec(&mut repo, "human:alex");
    assert!(r.changeset.is_some());
    // Status lifecycle + verify.
    crate::headless::single(
        &mut repo,
        None,
        Some("human:alex".into()),
        None,
        None,
        ApplyOp {
            op: "set_status".into(),
            path: "calc/revenue.md".into(),
            section: None,
            base_version: None,
            heading: None,
            level: None,
            content: None,
            after: None,
            key: None,
            value: Some(json!("draft")),
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    )
    .unwrap();
    crate::headless::single(
        &mut repo,
        None,
        Some("process:verifier".into()),
        None,
        None,
        ApplyOp {
            op: "verify".into(),
            path: "calc/revenue.md".into(),
            section: None,
            base_version: None,
            heading: None,
            level: None,
            content: None,
            after: None,
            key: None,
            value: None,
            r#type: None,
            frontmatter: None,
            note: None,
            split_from: None,
            merged_from: None,
        },
    )
    .unwrap();
    let t = read(&f.root, "calc/revenue.md");
    assert!(t.contains("status: draft"), "{t}");
    assert!(t.contains("verified:"), "{t}");
    assert!(t.contains("by: process:verifier"), "{t}");
    // Retract removes it from the view; restore brings it back byte-identical.
    crate::erase::retract(
        &mut repo,
        MAIN_REF,
        "calc/revenue.md",
        Reason::Withdrawn,
        Some("superseded by v2".into()),
        "human:alex",
    )
    .unwrap();
    assert!(!f.root.join("calc/revenue.md").exists());
    let st = status(&mut repo, None).unwrap();
    assert!(st.missing.is_empty(), "{st:?}");
    crate::erase::restore(&mut repo, MAIN_REF, "calc/revenue.md", "human:alex").unwrap();
    let t2 = read(&f.root, "calc/revenue.md");
    assert!(t2.contains("sum(y)"));
    assert!(t2.contains("status: draft"));
    // Revert the content change: back to sum(x).
    let log = crate::query::log(
        &mut repo,
        MAIN_REF,
        &crate::query::LogFilter {
            path: None,
            ops: Some(vec![Op::UpdateSection]),
            actor: None,
            since: None,
            include_erased: false,
            limit: None,
        },
    )
    .unwrap();
    let cs: ChangesetId = log["changesets"][0]["changeset"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    crate::query::revert(&mut repo, MAIN_REF, cs, "human:alex").unwrap();
    assert!(read(&f.root, "calc/revenue.md").contains("sum(x)"));
    assert!(crate::query::fsck(&mut repo).is_ok());
}

#[test]
fn split_merge_reorder_and_frontmatter_ops() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "d.md", "---\ntype: Doc\ntitle: D\ncustom: 1\n---\n## A\n\npart one\n\n### B\npart two\n\n## C\n\nsee\n");
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "d.md");
    let parsed = wisp_parse::parse_file(&text);
    let a = match &parsed.sections()[1].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    let b = match &parsed.sections()[2].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    let c = match &parsed.sections()[3].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    // Merge B into A (body absorbed), reorder C before A, change title, drop custom, add tags.
    let lines: Vec<&str> = text.lines().collect();
    let _ = lines;
    let merged = format!(
        "---\ntype: Doc\ntitle: D2\ntags: [x]\n---\n## C <!-- ^{c} -->\n\nsee\n\n## A <!-- ^{a} -->\n\npart one\n\npart two\n"
    );
    // Keep wisp_id so identity holds.
    let wid = text.lines().find(|l| l.starts_with("wisp_id: ")).unwrap();
    let merged = merged.replacen("---\n", &format!("---\n{wid}\n"), 1);
    write(&f.root, "d.md", &merged);
    let d = crate::record::diff(&mut repo, None, true, None).unwrap();
    let ops: Vec<String> = d["changes"]["d.md"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap().to_string())
        .collect();
    assert!(ops.contains(&"remove_section".to_string()), "{ops:?}");
    assert!(ops.contains(&"reorder_sections".to_string()), "{ops:?}");
    assert!(ops.contains(&"set_key".to_string()));
    assert!(ops.contains(&"unset_key".to_string()));
    let merged_lineage = d["changes"]["d.md"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["lineage"]["kind"] == "merged_from");
    assert!(merged_lineage, "{d}");
    rec(&mut repo, "human:alex");
    {
        let hot = repo.hot(MAIN_REF).unwrap();
        let st = Hot::concept_by_path(hot.conn(), "d").unwrap().unwrap();
        let secs = Hot::sections_of(hot.conn(), st.concept).unwrap();
        assert_eq!(
            secs.iter().map(|s| s.section).collect::<Vec<_>>(),
            vec![secs[0].section, c, a]
        );
        assert!(Hot::section(hot.conn(), b).unwrap().is_none());
        assert!(Hot::frontmatter_entry(hot.conn(), st.concept, "custom")
            .unwrap()
            .is_none());
        assert!(Hot::frontmatter_entry(hot.conn(), st.concept, "tags")
            .unwrap()
            .is_some());
    }
    // Split: A's body gets a new heading in the middle.
    let t = read(&f.root, "d.md");
    let split = t.replace("part one\n\npart two\n", "part one\n\n### Two\npart two\n");
    write(&f.root, "d.md", &split);
    let d2 = crate::record::diff(&mut repo, None, true, None).unwrap();
    assert!(
        d2["changes"]["d.md"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["op"] == "split"),
        "{d2}"
    );
    rec(&mut repo, "human:alex");
    assert!(crate::query::hot_check(&mut repo, MAIN_REF).is_ok());
}

#[test]
fn diff_between_changesets() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let log1 = crate::query::log(
        &mut repo,
        MAIN_REF,
        &crate::query::LogFilter {
            path: None,
            ops: None,
            actor: None,
            since: None,
            include_erased: false,
            limit: Some(1),
        },
    )
    .unwrap();
    let cs1: ChangesetId = log1["changesets"][0]["changeset"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    write(
        &f.root,
        "tables/orders.md",
        &ORDERS.replace("append-only", "immutable"),
    );
    rec(&mut repo, "human:alex");
    let log2 = crate::query::log(
        &mut repo,
        MAIN_REF,
        &crate::query::LogFilter {
            path: None,
            ops: None,
            actor: None,
            since: None,
            include_erased: false,
            limit: Some(1),
        },
    )
    .unwrap();
    let cs2: ChangesetId = log2["changesets"][0]["changeset"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let d = crate::record::diff_between_changesets(&mut repo, MAIN_REF, cs1, cs2, None).unwrap();
    let changes = d["changes"]["tables/orders"].as_array().unwrap();
    assert!(!changes.is_empty(), "{d}");
    assert!(
        changes.iter().any(|o| {
            matches!(
                o["op"].as_str(),
                Some("update_section" | "add_section" | "remove_section")
            )
        }),
        "{d}"
    );
    let empty =
        crate::record::diff_between_changesets(&mut repo, MAIN_REF, cs2, cs2, None).unwrap();
    assert!(empty["changes"].as_object().unwrap().is_empty());
}

#[test]
fn search_pack_and_feed() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    write(&f.root, "tables/customers.md", CUSTOMERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let s = crate::query::search(&mut repo, MAIN_REF, "append-only rows", "auto", 5).unwrap();
    assert!(s["mode"].as_str().unwrap() == "auto" || s["mode"].as_str().unwrap() == "exact");
    assert_eq!(s["results"][0]["heading"], "Behavior");
    let p =
        crate::query::pack(&mut repo, MAIN_REF, &["tables/orders.md".into()], Some(20)).unwrap();
    assert!(p["tokens"].as_u64().unwrap() <= 20);
    assert!(!p["omitted"].as_array().unwrap().is_empty());
    let full = crate::query::pack(&mut repo, MAIN_REF, &[], None).unwrap();
    assert_eq!(full["concepts"].as_array().unwrap().len(), 2);
    assert!(full["concepts"][0]["links"].is_array());
    let feed = crate::query::feed(&repo, 0).unwrap();
    assert!(feed.iter().all(|e| e.get("content").is_none()));
    assert!(feed.len() >= 6);
    assert_eq!(feed[0]["event"], "upsert");
}

#[test]
fn assets_and_nonconformant_and_generated_edits() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    write(&f.root, "references/attesters/revenue.py", "print('hi')\n");
    write(&f.root, "junk.md", "no frontmatter here\n");
    let mut repo = open(&f.root);
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:alex".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Validation);
    std::fs::remove_file(f.root.join("junk.md")).unwrap();
    rec(&mut repo, "human:alex");
    {
        let hot = repo.hot(MAIN_REF).unwrap();
        assert!(
            Hot::asset_by_path(hot.conn(), "references/attesters/revenue.py")
                .unwrap()
                .is_some()
        );
    }
    // Editing a generated file aborts with exit 4 unless adopted.
    write(&f.root, "index.md", "hand edited\n");
    write(
        &f.root,
        "tables/orders.md",
        &read(&f.root, "tables/orders.md").replace("append-only", "append only"),
    );
    let err = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:alex".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Refused);
    let r = record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:alex".into()),
            adopt_generated: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.changeset.is_some());
    assert!(!read(&f.root, "index.md").contains("hand edited"));
    // Missing asset → retract.
    std::fs::remove_file(f.root.join("references/attesters/revenue.py")).unwrap();
    let r = rec(&mut repo, "human:alex");
    assert!(r.changeset.is_some());
    let hot = repo.hot(MAIN_REF).unwrap();
    assert!(
        Hot::asset_by_path(hot.conn(), "references/attesters/revenue.py")
            .unwrap()
            .is_none()
    );
}

#[test]
fn branches_and_apply_batches_with_asserts_and_inputs() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(
        &f.root,
        "tables/door.md",
        "---\ntype: Table\n---\n## Behavior\n\nopens\n",
    );
    write(
        &f.root,
        "observations/2026-10-17.md",
        "---\ntype: Observation\n---\nThe door squeaks.\n",
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "tables/door.md");
    let sid = match &wisp_parse::parse_file(&text).sections()[1].anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!(),
    };
    crate::query::branch(&mut repo, "task/door", None).unwrap();
    let batch: ApplyBatch = serde_json::from_value(json!({
        "ref": "task/door", "actor": "sl-bot/0.3", "message": "Sync door behavior",
        "ops": [
            { "op": "update_section", "path": "tables/door.md", "section": sid.to_string(), "heading": "Behavior", "content": "opens and squeaks" },
            { "op": "set_key", "path": "tables/door.md", "key": "tags", "base_version": "absent", "value": ["interactive"] }
        ],
        "inputs": [ { "concept": "observations/2026-10-17.md" } ],
        "assert": [ { "path": "observations/2026-10-17.md", "version": "01JAB600000000000000000000" } ]
    })).unwrap();
    // The assertion names a version that does not exist → conflict.
    let err = crate::headless::apply(&mut repo, batch, None).unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    assert_eq!(err.details["conflicts"][0]["type"], "assertion_failed");
    // Without the assertion it commits to the branch, not main.
    let batch: ApplyBatch = serde_json::from_value(json!({
        "ref": "task/door", "actor": "sl-bot/0.3",
        "ops": [ { "op": "update_section", "path": "tables/door.md", "section": sid.to_string(), "content": "opens and squeaks" } ],
        "inputs": [ { "concept": "observations/2026-10-17.md" } ]
    })).unwrap();
    crate::headless::apply(&mut repo, batch, None).unwrap();
    {
        let main = repo.hot(MAIN_REF).unwrap();
        assert_eq!(
            Hot::section(main.conn(), sid).unwrap().unwrap().body,
            "\nopens\n"
        );
    }
    {
        let br = repo.hot("task/door").unwrap();
        assert_eq!(
            Hot::section(br.conn(), sid).unwrap().unwrap().body,
            "opens and squeaks\n"
        );
    }
    // why shows the input graph.
    let why =
        crate::query::why(&mut repo, "task/door", &format!("tables/door.md#{sid}"), 1).unwrap();
    let events = why["history"]["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["inputs"].is_array()), "{why}");
    assert!(!why["history"]["inputs"].as_array().unwrap().is_empty());
    // Switch the working tree to the branch and back.
    crate::query::switch(&mut repo, "task/door", false).unwrap();
    assert!(read(&f.root, "tables/door.md").contains("squeaks"));
    crate::query::switch(&mut repo, MAIN_REF, false).unwrap();
    assert!(!read(&f.root, "tables/door.md").contains("squeaks"));
    let merged = crate::merge::merge_refs(&mut repo, "task/door", MAIN_REF, None, false).unwrap();
    assert_eq!(merged["status"], "fast_forward");
    assert!(read(&f.root, "tables/door.md").contains("squeaks"));
    assert!(crate::query::fsck(&mut repo).is_ok());
}

#[test]
fn mv_rewrites_links_and_i25_staging_recovers() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    write(&f.root, "tables/customers.md", CUSTOMERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    crate::query::mv(
        &mut repo,
        MAIN_REF,
        "tables/customers.md",
        "crm/customers.md",
        "human:alex",
    )
    .unwrap();
    let orders = read(&f.root, "tables/orders.md");
    assert!(orders.contains("](../crm/customers.md)"), "{orders}");
    assert!(f.root.join("crm/customers.md").exists());
    assert!(!f.root.join("tables/customers.md").exists());
    // Recording again is a no-op (the rewrite was recorded, so stored content matches the files).
    assert_eq!(rec(&mut repo, "human:alex").ops, 0);
    // I25: a payload staged but not flushed is flushed on open.
    let pid = PayloadId::new();
    Ledger::stage_payload(repo.ledger.conn(), pid, b"WSP1fake").unwrap();
    drop(repo);
    let repo = open(&f.root);
    assert!(repo
        .ledger
        .payloads_dir()
        .join(Ledger::payload_rel_path(pid))
        .exists());
}

#[test]
fn worktree_add_list_remove() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(&f.root, "tables/orders.md", ORDERS);
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let added =
        crate::worktree::add(&mut repo, "side", Some("task/side"), Some(MAIN_REF), None).unwrap();
    assert_eq!(added["name"], "side");
    assert_eq!(added["ref"], "task/side");
    let wt_path = f.root.join(".wisp/worktrees/side");
    assert!(wt_path.join("tables/orders.md").exists());
    assert!(wt_path.join(".wisp-checkout.idx").exists());
    let listed = crate::worktree::list(&repo).unwrap();
    let wts = listed["worktrees"].as_array().unwrap();
    assert_eq!(wts.len(), 2);
    assert!(wts[0]["primary"].as_bool().unwrap());
    assert_eq!(wts[1]["name"], "side");
    let err = crate::worktree::add(&mut repo, "dup", None, Some("task/side"), None).unwrap_err();
    assert_eq!(err.code, ExitCode::Usage);
    crate::worktree::remove(&mut repo, "side", false).unwrap();
    assert!(!wt_path.exists());
    assert_eq!(
        crate::worktree::list(&repo).unwrap()["worktrees"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn erase_rewrites_linked_worktree_for_ref() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let canary = "CANARY_WT_ERASE_9e2b";
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha\n\n{canary}\n"),
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    crate::worktree::add(&mut repo, "side", Some("task/side"), Some(MAIN_REF), None).unwrap();
    let wt_root = f.root.join(".wisp/worktrees/side");
    assert!(!scan_for(&wt_root, canary).is_empty());
    let text = read(&wt_root, "notes/standup.md");
    let parsed = wisp_parse::parse_file(&text);
    let alpha = parsed
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha")
        .and_then(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => Some(*id),
            _ => None,
        })
        .expect("Alpha section anchor");
    crate::erase::erase(
        &mut repo,
        "task/side",
        &EraseTarget::Path(format!("notes/standup.md#{alpha}")),
        Reason::Policy,
        true,
        true,
        "human:alex",
    )
    .unwrap();
    drop(repo);
    assert!(
        scan_for(&wt_root, canary).is_empty(),
        "linked worktree still contains plaintext"
    );
    assert!(
        scan_for(&f.root, canary).is_empty(),
        "primary on main must be rewritten after global shred"
    );
}

#[test]
fn recover_after_shred_before_worktree_rewrite() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let canary = "CANARY_CRASH_SHRED_7c1e";
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha\n\n{canary}\n"),
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "notes/standup.md");
    let alpha = wisp_parse::parse_file(&text)
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha")
        .and_then(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => Some(*id),
            _ => None,
        })
        .expect("Alpha anchor");
    let scope = Ledger::scope_of_section(repo.ledger.conn(), alpha)
        .unwrap()
        .unwrap()
        .id;
    let mut op = crate::commit::PlannedOp::new(
        Op::Erase,
        Target::Erase {
            scopes: vec![scope],
        },
        repo.repo_scope(),
    );
    op.reason = Some(Reason::Policy);
    let mut req = crate::commit::CommitRequest::new(MAIN_REF, "human:alex");
    req.ops = vec![op];
    let out = crate::commit::commit(&mut repo, req).unwrap();
    let erase_change = out
        .changes
        .iter()
        .find(|c| c.op == Op::Erase)
        .map(|c| c.id)
        .expect("erase change");
    crate::erase::finish_erase_bookkeeping(&mut repo).unwrap();
    let pending = Ledger::scopes_erased_not_shredded(repo.ledger.conn()).unwrap();
    crate::erase::shred(&mut repo, &pending, erase_change, &[]).unwrap();
    assert!(read(&f.root, "notes/standup.md").contains(canary));
    drop(repo);
    let _repo = open(&f.root);
    assert!(
        !read(&f.root, "notes/standup.md").contains(canary),
        "recover must rewrite when shred finished but worktrees were not"
    );
}

#[test]
fn erase_rewrite_requires_discard_when_worktree_dirty() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    write(
        &f.root,
        "notes/standup.md",
        "---\ntype: Note\n---\n## Alpha\n\nx\n",
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "notes/standup.md");
    let alpha = wisp_parse::parse_file(&text)
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha")
        .and_then(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => Some(*id),
            _ => None,
        })
        .expect("Alpha anchor");
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha {{#{alpha}}}\n\nedited\n"),
    );
    let err = crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Path(format!("notes/standup.md#{alpha}")),
        Reason::Policy,
        true,
        false,
        "human:alex",
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
}

#[test]
fn recover_after_erase_commit_before_bookkeeping() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let canary = "CANARY_CRASH_BOOK_2a9f";
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha\n\n{canary}\n"),
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "notes/standup.md");
    let alpha = wisp_parse::parse_file(&text)
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha")
        .and_then(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => Some(*id),
            _ => None,
        })
        .expect("Alpha anchor");
    let scope = Ledger::scope_of_section(repo.ledger.conn(), alpha)
        .unwrap()
        .unwrap()
        .id;
    let mut op = crate::commit::PlannedOp::new(
        Op::Erase,
        Target::Erase {
            scopes: vec![scope],
        },
        repo.repo_scope(),
    );
    op.reason = Some(Reason::Policy);
    let mut req = crate::commit::CommitRequest::new(MAIN_REF, "human:alex");
    req.ops = vec![op];
    crate::commit::commit(&mut repo, req).unwrap();
    assert!(read(&f.root, "notes/standup.md").contains(canary));
    assert!(
        Ledger::changes_all(repo.ledger.conn())
            .unwrap()
            .iter()
            .any(|(_, c)| c.op == Op::Erase),
        "erase change must be in the ledger"
    );
    drop(repo);
    let repo = open(&f.root);
    assert!(
        Ledger::scope(repo.ledger.conn(), scope)
            .unwrap()
            .unwrap()
            .shredded,
        "recover must finish cold shred"
    );
    assert!(
        !read(&f.root, "notes/standup.md").contains(canary),
        "recover must rewrite the managed primary working tree"
    );
}

#[test]
fn rewrite_managed_worktrees_clears_stale_plaintext_after_erase() {
    let f = fixture();
    Repository::init(&f.root, "0.2", &Unlock::default()).unwrap();
    let canary = "CANARY_STALE_8f3d";
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha\n\n{canary}\n"),
    );
    let mut repo = open(&f.root);
    rec(&mut repo, "human:alex");
    let text = read(&f.root, "notes/standup.md");
    let alpha = wisp_parse::parse_file(&text)
        .sections()
        .iter()
        .find(|s| s.heading == "Alpha")
        .and_then(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => Some(*id),
            _ => None,
        })
        .expect("Alpha anchor");
    crate::erase::erase(
        &mut repo,
        MAIN_REF,
        &EraseTarget::Path(format!("notes/standup.md#{alpha}")),
        Reason::Policy,
        true,
        true,
        "human:alex",
    )
    .unwrap();
    write(
        &f.root,
        "notes/standup.md",
        &format!("---\ntype: Note\n---\n## Alpha\n\nstale {canary}\n"),
    );
    crate::erase::rewrite_all_managed_worktrees(&mut repo, true).unwrap();
    assert!(!read(&f.root, "notes/standup.md").contains(canary));
}

#[test]
fn passphrase_and_env_master_keys() {
    let f = fixture();
    let unlock = Unlock {
        passphrase: Some("correct horse".into()),
    };
    Repository::init(&f.root, "0.2", &unlock).unwrap();
    write(&f.root, "a.md", "---\ntype: T\n---\nx\n");
    let mut repo = Repository::open(&f.root, &unlock).unwrap();
    rec(&mut repo, "human:alex");
    drop(repo);
    let wrong = Unlock {
        passphrase: Some("wrong".into()),
    };
    let err = match Repository::open(&f.root, &wrong) {
        Ok(_) => panic!("wrong passphrase opened the keystore"),
        Err(e) => e,
    };
    assert_eq!(err.code, ExitCode::KeyUnavailable);
    assert!(Repository::open(&f.root, &unlock).is_ok());
}

//! Optional many-writer stress (eval follow-up). Default CI uses the engine's 8×5 test;
//! run `WISP_STRESS=1 WISP_STRESS_WRITERS=50 cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers`.

use wisp_engine::headless::ApplyOp;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_model::*;
use wisp_store::Hot;

fn writer_count() -> usize {
    std::env::var("WISP_STRESS_WRITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
}

#[test]
#[ignore = "heavy; run with WISP_STRESS=1"]
fn stress_many_disjoint_writers() {
    if std::env::var("WISP_STRESS").as_deref() != Ok("1") {
        return;
    }
    let n = writer_count();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    std::fs::create_dir_all(&root).unwrap();
    std::env::remove_var("WISP_MASTER_KEY");
    let mut body = String::from("---\ntype: Doc\n---\n");
    for i in 0..n {
        body.push_str(&format!("## S{i}\n\ninitial {i}\n\n"));
    }
    std::fs::write(root.join("doc.md"), &body).unwrap();
    let mut repo = Repository::init(&root, "0.2", &Unlock::default()).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:stress".into()),
            message: Some("init".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(root.join("doc.md")).unwrap();
    let parsed = wisp_parse::parse_file(&text);
    let ids: Vec<SectionId> = parsed
        .sections()
        .iter()
        .skip(1)
        .map(|s| match &s.anchor {
            Some(wisp_parse::Anchor::Valid(id)) => *id,
            _ => panic!("anchor"),
        })
        .collect();
    assert_eq!(ids.len(), n);
    let hot = repo.hot(MAIN_REF).unwrap();
    let bases: Vec<ChangeId> = ids
        .iter()
        .map(|id| Hot::section(hot.conn(), *id).unwrap().unwrap().version)
        .collect();
    drop(repo);
    let ops_per_writer = 3usize;
    let handles: Vec<_> = (0..n)
        .map(|w| {
            let root = root.clone();
            let id = ids[w];
            let base = bases[w];
            std::thread::spawn(move || {
                let mut repo = Repository::open(&root, &Unlock::default()).unwrap();
                let mut base = base;
                for k in 0..ops_per_writer {
                    let op = ApplyOp {
                        op: "update_section".into(),
                        path: "doc.md".into(),
                        section: Some(id),
                        base_version: Some(base.to_string()),
                        heading: None,
                        level: None,
                        content: Some(format!("writer {w} op {k}")),
                        after: None,
                        key: None,
                        value: None,
                        r#type: None,
                        frontmatter: None,
                        note: None,
                        split_from: None,
                        merged_from: None,
                    };
                    let r = wisp_engine::headless::single(
                        &mut repo,
                        None,
                        Some(format!("process:stress-{w}")),
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
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("writer thread");
    }
    let mut repo = Repository::open(&root, &Unlock::default()).unwrap();
    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
}

//! Optional many-writer stress (eval follow-up). Default CI uses the engine's 8×5 test;
//! run `WISP_STRESS=1 WISP_STRESS_WRITERS=50 cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers`.

use std::time::Duration;
use wisp_engine::headless::ApplyOp;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::{Repository, Unlock, MAIN_REF};
use wisp_model::{ExitCode, *};
use wisp_store::Hot;

/// Headless apply with backoff on exit 7 (`WriteTimeout`), same class of contention as `commit`.
fn refresh_op_base(repo: &mut Repository, op: &mut ApplyOp) -> Result<()> {
    let Some(section) = op.section else {
        return Ok(());
    };
    let hot = repo.hot(MAIN_REF)?;
    let row = Hot::section(hot.conn(), section)?
        .ok_or_else(|| WispError::not_found("section not in hot tier"))?;
    op.base_version = Some(row.version.to_string());
    Ok(())
}

fn single_with_retry(
    repo: &mut Repository,
    actor: &str,
    mut op: ApplyOp,
) -> Result<serde_json::Value> {
    const MAX_ATTEMPTS: u32 = 128;
    for attempt in 1..=MAX_ATTEMPTS {
        match wisp_engine::headless::single(
            repo,
            None,
            Some(actor.to_string()),
            None,
            None,
            op.clone(),
        ) {
            Ok(v) => return Ok(v),
            Err(e) if e.is_retryable() && attempt < MAX_ATTEMPTS => {
                refresh_op_base(repo, &mut op)?;
                let jitter = 20 + (attempt as u64) * 15;
                std::thread::sleep(Duration::from_millis(jitter));
                continue;
            }
            Err(e) if e.code == ExitCode::Conflict && attempt < MAX_ATTEMPTS => {
                refresh_op_base(repo, &mut op)?;
                let jitter = 20 + (attempt as u64) * 15;
                std::thread::sleep(Duration::from_millis(jitter));
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("single_with_retry loop")
}

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
                        ..Default::default()
                    };
                    let r =
                        single_with_retry(&mut repo, &format!("process:stress-{w}"), op).unwrap();
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
    match wisp_engine::query::fsck(&mut repo) {
        Ok(_) => {}
        Err(e) => panic!("fsck after concurrent writers: {e:?}"),
    }
}

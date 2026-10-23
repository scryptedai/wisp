//! Input version capture: `--input <path>#<section>@<changeset>` pins, their validation, and the
//! structured warning for non-human content writes that declare no inputs.

mod impact_harness;

use impact_harness::*;
use serde_json::json;
use wisp_engine::headless::parse_input_spec;
use wisp_model::{ExitCode, InputRef};

const DOC: &str = "---\ntype: Source\n---\n## Retries\n\nRetry 5 times.\n";
const NOTE: &str = "---\ntype: Note\n---\n## Policy\n\nTBD\n";

#[test]
fn parse_input_spec_forms() {
    let p = parse_input_spec("sources/doc.md").unwrap();
    assert_eq!(p.concept, "sources/doc.md");
    assert!(p.section.is_none() && p.at.is_none());

    let s = "01JAD2V7000000000000000001";
    let cs = "01JA9Z0T000000000000000001";
    let p = parse_input_spec(&format!("sources/doc.md#{s}")).unwrap();
    assert_eq!(p.section.unwrap().to_string(), s);
    assert!(p.at.is_none());

    let p = parse_input_spec(&format!("sources/doc.md#{s}@{cs}")).unwrap();
    assert_eq!(p.at.unwrap().to_string(), cs);

    // A malformed section id used to degrade silently to a whole-concept input.
    let e = parse_input_spec("sources/doc.md#not-a-section").unwrap_err();
    assert_eq!(e.code, ExitCode::Usage);
    let e = parse_input_spec(&format!("sources/doc.md#{s}@nope")).unwrap_err();
    assert_eq!(e.code, ExitCode::Usage);
}

#[test]
fn pinned_input_is_recorded_and_unpinned_stays_unpinned() {
    let (_d, _root, mut repo) = fresh(&[("sources/doc.md", DOC), ("notes/note.md", NOTE)]);
    let m = main_ref();
    let read_at = head(&mut repo, m);
    let pinned = input(
        &mut repo,
        m,
        "sources/doc.md",
        Some("Retries"),
        Some(read_at),
    );
    let r = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "Retry 5 times.\n",
        json!([pinned]),
        json!({}),
    )
    .unwrap();
    assert!(r.get("warnings").is_none(), "{r}");
    let v = written_version(&r);
    match &ledger_inputs(&repo, v)[0] {
        InputRef::Concept { at, section, .. } => {
            assert_eq!(*at, Some(read_at));
            assert!(section.is_some());
        }
        other => panic!("unexpected input {other:?}"),
    }

    let unpinned = input(&mut repo, m, "sources/doc.md", Some("Retries"), None);
    let r = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "Retry 5 times, backoff.\n",
        json!([unpinned]),
        json!({}),
    )
    .unwrap();
    match &ledger_inputs(&repo, written_version(&r))[0] {
        InputRef::Concept { at, .. } => assert_eq!(*at, None, "never inferred from head"),
        other => panic!("unexpected input {other:?}"),
    }
    let fsck = wisp_engine::query::fsck(&mut repo).unwrap();
    assert!(fsck["problems"].as_array().unwrap().is_empty(), "{fsck}");
}

#[test]
fn invalid_pins_are_rejected() {
    let (_d, _root, mut repo) = fresh(&[("sources/doc.md", DOC), ("notes/note.md", NOTE)]);
    let m = main_ref();
    let before = head(&mut repo, m);

    // Unknown changeset.
    let mut bad = input(&mut repo, m, "sources/doc.md", Some("Retries"), None);
    bad["at"] = json!("01JA9Z0T000000000000000001");
    let e = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "x\n",
        json!([bad]),
        json!({}),
    )
    .unwrap_err();
    assert_eq!(e.code, ExitCode::NotFound, "{e:?}");

    // A section version (ChangeId) passed where a snapshot (ChangesetId) belongs.
    let (_, doc_version) = section(&mut repo, m, "sources/doc.md", "Retries");
    let mut bad = input(&mut repo, m, "sources/doc.md", Some("Retries"), None);
    bad["at"] = json!(doc_version.to_string());
    let e = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "x\n",
        json!([bad]),
        json!({}),
    )
    .unwrap_err();
    assert_eq!(e.code, ExitCode::Usage);
    assert!(e.message.contains("not a changeset"), "{}", e.message);

    // A section added after the pinned snapshot does not resolve there.
    let (doc_sid, _) = section(&mut repo, m, "sources/doc.md", "Retries");
    let added = apply_json(
        &mut repo,
        json!({
            "ref": m, "actor": HUMAN,
            "ops": [{ "op": "add_section", "path": "sources/doc.md", "after": doc_sid.to_string(),
                      "heading": "Webhooks", "content": "Sign every payload.\n" }]
        }),
    )
    .unwrap();
    assert!(added.get("warnings").is_none(), "human writes never warn");
    let after_add = head(&mut repo, m);
    let mut bad = input(&mut repo, m, "sources/doc.md", Some("Webhooks"), None);
    bad["at"] = json!(before.to_string());
    let e = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "x\n",
        json!([bad]),
        json!({}),
    )
    .unwrap_err();
    assert_eq!(e.code, ExitCode::NotFound);
    assert!(
        e.message.contains("does not exist at that snapshot"),
        "{}",
        e.message
    );

    // Rejected writes commit nothing.
    assert_eq!(head(&mut repo, m), after_add);
    // The same section pinned at a snapshot where it exists is accepted.
    let mut ok = input(&mut repo, m, "sources/doc.md", Some("Webhooks"), None);
    ok["at"] = json!(after_add.to_string());
    update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "x\n",
        json!([ok]),
        json!({}),
    )
    .unwrap();
}

#[test]
fn missing_inputs_warning_is_structured_and_never_a_refusal() {
    let (_d, root, mut repo) = fresh(&[("sources/doc.md", DOC), ("notes/note.md", NOTE)]);
    let m = main_ref();

    let r = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "From memory.\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    let w = &r["warnings"][0];
    assert_eq!(w["code"], "no_declared_inputs", "{r}");
    assert_eq!(w["actor"], AGENT);
    assert_eq!(w["sections"].as_array().unwrap().len(), 1);

    let r = update(
        &mut repo,
        m,
        "process:indexer",
        "notes/note.md",
        "Policy",
        "Again.\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    assert_eq!(r["warnings"][0]["code"], "no_declared_inputs");

    let r = update(
        &mut repo,
        m,
        HUMAN,
        "notes/note.md",
        "Policy",
        "Human edit.\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    assert!(r.get("warnings").is_none());

    // `record` by a non-human actor warns too.
    write(
        &root,
        "notes/note.md",
        &std::fs::read_to_string(root.join("notes/note.md"))
            .unwrap()
            .replace("Human edit.", "Recorded edit."),
    );
    let rr = wisp_engine::record::record(
        &mut repo,
        &wisp_engine::record::RecordOptions {
            actor: Some(AGENT.into()),
            message: Some("agent record".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rr.changeset.is_some());
    assert!(
        rr.warnings
            .iter()
            .any(|w| w["code"] == "no_declared_inputs"),
        "{:?}",
        rr.warnings
    );
}

#[test]
fn per_op_inputs_override_batch_inputs() {
    const NOTE2: &str = "---\ntype: Note\n---\n## Policy\n\nTBD\n\n## Errors\n\nTBD\n";
    let (_d, _root, mut repo) = fresh(&[
        ("sources/doc.md", DOC),
        (
            "sources/auth.md",
            "---\ntype: Source\n---\n## Tokens\n\nRotate daily.\n",
        ),
        ("notes/note.md", NOTE2),
    ]);
    let m = main_ref();
    let at = head(&mut repo, m);
    let doc = input(&mut repo, m, "sources/doc.md", Some("Retries"), Some(at));
    let auth = input(&mut repo, m, "sources/auth.md", Some("Tokens"), Some(at));
    let (policy, pv) = section(&mut repo, m, "notes/note.md", "Policy");
    let (errors, ev) = section(&mut repo, m, "notes/note.md", "Errors");
    let r = apply_json(
        &mut repo,
        json!({
            "ref": m, "actor": AGENT, "inputs": [doc],
            "ops": [
                { "op": "update_section", "path": "notes/note.md", "section": policy.to_string(),
                  "base_version": pv.to_string(), "content": "Retry 5 times.\n" },
                { "op": "update_section", "path": "notes/note.md", "section": errors.to_string(),
                  "base_version": ev.to_string(), "content": "Refresh tokens.\n", "inputs": [auth] }
            ]
        }),
    )
    .unwrap();
    let (doc_sid, _) = section(&mut repo, m, "sources/doc.md", "Retries");
    let (auth_sid, _) = section(&mut repo, m, "sources/auth.md", "Tokens");
    let versions: Vec<(String, wisp_model::ChangeId)> = r["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("section").is_some())
        .map(|c| {
            (
                c["section"].as_str().unwrap().to_string(),
                c["version"].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect();
    let inputs_of = |sid: wisp_model::SectionId| {
        let v = versions
            .iter()
            .find(|(s, _)| *s == sid.to_string())
            .unwrap()
            .1;
        ledger_inputs(&repo, v)
    };
    let sec_of = |i: &InputRef| match i {
        InputRef::Concept { section, .. } => section.unwrap(),
        _ => panic!(),
    };
    let p = inputs_of(policy);
    assert_eq!(p.len(), 1);
    assert_eq!(sec_of(&p[0]), doc_sid, "batch-level inputs are the default");
    let e = inputs_of(errors);
    assert_eq!(
        e.len(),
        1,
        "per-op inputs replace, not extend, the batch inputs"
    );
    assert_eq!(sec_of(&e[0]), auth_sid);

    // An explicit empty per-op list declares no inputs for that op only.
    let (policy, pv) = section(&mut repo, m, "notes/note.md", "Policy");
    let (errors, ev) = section(&mut repo, m, "notes/note.md", "Errors");
    let doc = input(&mut repo, m, "sources/doc.md", Some("Retries"), None);
    let r = apply_json(
        &mut repo,
        json!({
            "ref": m, "actor": AGENT, "inputs": [doc],
            "ops": [
                { "op": "update_section", "path": "notes/note.md", "section": policy.to_string(),
                  "base_version": pv.to_string(), "content": "Retry 6 times.\n" },
                { "op": "update_section", "path": "notes/note.md", "section": errors.to_string(),
                  "base_version": ev.to_string(), "content": "Typo fix.\n", "inputs": [] }
            ]
        }),
    )
    .unwrap();
    let w = r["warnings"][0]["sections"].as_array().unwrap();
    assert_eq!(w, &vec![json!(errors.to_string())], "{r}");
}

#[test]
fn inputs_complete_is_stored_hashed_and_migrates_old_ledgers() {
    let (_d, root, mut repo) = fresh(&[("sources/doc.md", DOC), ("notes/note.md", NOTE)]);
    let m = main_ref();
    let at = head(&mut repo, m);
    let pinned = input(&mut repo, m, "sources/doc.md", Some("Retries"), Some(at));
    let r = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "Rewritten from the doc.\n",
        json!([pinned]),
        json!({ "inputs_complete": true }),
    )
    .unwrap();
    let v = written_version(&r);
    let ch = wisp_store::Ledger::get_change(repo.ledger.conn(), v)
        .unwrap()
        .unwrap();
    assert!(ch.inputs_complete);

    // An explicit "complete and empty" declaration is a declaration: no warning.
    let r = update(
        &mut repo,
        m,
        AGENT,
        "notes/note.md",
        "Policy",
        "Original observation.\n",
        json!([]),
        json!({ "op.inputs_complete": true }),
    )
    .unwrap();
    assert!(r.get("warnings").is_none(), "{r}");
    let plain = update(
        &mut repo,
        m,
        HUMAN,
        "notes/note.md",
        "Policy",
        "Human.\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    let pv = written_version(&plain);
    assert!(
        !wisp_store::Ledger::get_change(repo.ledger.conn(), pv)
            .unwrap()
            .unwrap()
            .inputs_complete
    );
    let fsck = wisp_engine::query::fsck(&mut repo).unwrap();
    assert!(fsck["problems"].as_array().unwrap().is_empty(), "{fsck}");
    drop(repo);

    // A ledger from an earlier build has no inputs_complete column. Simulate it by dropping the
    // column from a copy that only holds unflagged records, then reopen: the column is added
    // back and every digest still verifies.
    let (_d2, root2, repo2) = fresh(&[("sources/doc.md", DOC), ("notes/note.md", NOTE)]);
    drop(repo2);
    {
        let conn = rusqlite::Connection::open(root2.join(".wisp/ledger.db")).unwrap();
        conn.execute_batch("ALTER TABLE changes DROP COLUMN inputs_complete")
            .unwrap();
    }
    let mut reopened = wisp_engine::Repository::open(&root2, &unlock()).unwrap();
    let fsck = wisp_engine::query::fsck(&mut reopened).unwrap();
    assert!(fsck["problems"].as_array().unwrap().is_empty(), "{fsck}");
    let _ = root;
}

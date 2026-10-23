//! `wisp retract` reports recorded direct dependents (read-only), like `wisp erase`'s `flagged`.

mod impact_harness;

use impact_harness::*;
use serde_json::json;
use wisp_model::Reason;

#[test]
fn retract_lists_recorded_dependents() {
    let (_d, _root, mut repo) = fresh(&[
        (
            "sources/doc.md",
            "---\ntype: Source\n---\n## Retries\n\nRetry 5 times.\n",
        ),
        (
            "notes/derived.md",
            "---\ntype: Note\n---\n## Policy\n\nTBD\n",
        ),
        ("notes/other.md", "---\ntype: Note\n---\n## Policy\n\nTBD\n"),
    ]);
    let m = main_ref();
    let at = head(&mut repo, m);
    let doc = input(&mut repo, m, "sources/doc.md", Some("Retries"), Some(at));
    update(
        &mut repo,
        m,
        AGENT,
        "notes/derived.md",
        "Policy",
        "Retry 5x.\n",
        json!([doc]),
        json!({}),
    )
    .unwrap();
    update(
        &mut repo,
        m,
        AGENT,
        "notes/other.md",
        "Policy",
        "Unrelated.\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    let derived = concept_id(&mut repo, m, "notes/derived.md");
    let ledger_seq_before = wisp_store::Ledger::max_seq(repo.ledger.conn()).unwrap();
    let r = wisp_engine::erase::retract(
        &mut repo,
        m,
        "sources/doc.md",
        Reason::SourceWithdrawn,
        None,
        HUMAN,
    )
    .unwrap();
    assert_eq!(r["flagged"], json!([derived.to_string()]), "{r}");
    // Exactly one changeset: the retraction. The dependents lookup wrote nothing.
    assert_eq!(
        wisp_store::Ledger::max_seq(repo.ledger.conn()).unwrap(),
        ledger_seq_before + 1
    );
}

//! `wisp why` is scoped to the selected ref, like `wisp impact`.

mod impact_harness;

use impact_harness::*;
use serde_json::json;

#[test]
fn why_history_is_scoped_to_the_selected_ref() {
    let (_d, _root, mut repo) = fresh(&[
        (
            "sources/doc.md",
            "---\ntype: Source\n---\n## Retries\n\nRetry 5 times.\n",
        ),
        ("notes/note.md", "---\ntype: Note\n---\n## Policy\n\nTBD\n"),
    ]);
    let m = main_ref();
    wisp_engine::query::branch(&mut repo, "feature", None).unwrap();
    let doc = input(
        &mut repo,
        "feature",
        "sources/doc.md",
        Some("Retries"),
        None,
    );
    update(
        &mut repo,
        "feature",
        AGENT,
        "notes/note.md",
        "Policy",
        "Branch only.\n",
        json!([doc]),
        json!({}),
    )
    .unwrap();
    let (sid, _) = section(&mut repo, m, "notes/note.md", "Policy");
    let spec = format!("notes/note.md#{sid}");
    let events = |v: &serde_json::Value| v["history"]["events"].as_array().unwrap().len();

    let on_main = wisp_engine::query::why(&mut repo, m, &spec, 1).unwrap();
    let on_feature = wisp_engine::query::why(&mut repo, "feature", &spec, 1).unwrap();
    assert_eq!(on_main["ref"], m);
    assert_eq!(
        events(&on_feature),
        events(&on_main) + 1,
        "{on_main}\n{on_feature}"
    );
    assert!(on_main["history"]["inputs"].as_array().unwrap().is_empty());
    assert!(!on_feature["history"]["inputs"]
        .as_array()
        .unwrap()
        .is_empty());
}

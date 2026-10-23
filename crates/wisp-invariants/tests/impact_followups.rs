//! Follow-ups to `wisp impact`: the `inputs_complete` format marker, frontmatter inheritance,
//! repository-wide erase dependents, and the union rule for identical-content merges.

mod impact_harness;

use impact_harness::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use wisp_engine::erase::EraseTarget;
use wisp_engine::impact::impact;
use wisp_engine::manifest::{Manifest, FORMAT_BASE, FORMAT_INPUTS_COMPLETE};
use wisp_engine::repo::Repository;
use wisp_model::{ChangeId, ExitCode, Reason};

const SRC: &str = "---\ntype: Source\n---\n## S\n\nsource\n";
const SRC2: &str = "---\ntype: Source\n---\n## S\n\nother source\n";
const NOTE: &str = "---\ntype: Note\n---\n## A\n\nTBD\n";

fn format_of(root: &std::path::Path) -> u32 {
    Manifest::load(&root.join(".wisp/manifest.toml"))
        .unwrap()
        .repo
        .format
}

fn pinned(repo: &mut Repository, ref_name: &str, path: &str, heading: &str) -> Value {
    let at = head(repo, ref_name);
    input(repo, ref_name, path, Some(heading), Some(at))
}

fn versions(v: &Value) -> BTreeSet<String> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["version"].as_str().unwrap().to_string())
        .collect()
}

fn result_for(v: &Value, version: ChangeId) -> Value {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["version"] == json!(version.to_string()))
        .cloned()
        .unwrap_or_else(|| panic!("no result for {version} in {v:#}"))
}

#[test]
fn format_marker_is_written_on_first_use_and_newer_formats_refuse() {
    let (_d, root, mut repo) = fresh(&[("sources/src.md", SRC), ("notes/a.md", NOTE)]);
    let m = main_ref();
    // Ordinary writes, including pinned inputs, keep the base format.
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    update(
        &mut repo,
        m,
        AGENT,
        "notes/a.md",
        "A",
        "x\n",
        json!([i]),
        json!({}),
    )
    .unwrap();
    assert_eq!(format_of(&root), FORMAT_BASE);

    // A trusted replica made now, before the flag is used.
    let replica_dir = tempfile::tempdir().unwrap();
    let replica = replica_dir.path().join("replica");
    drop(repo);
    drop(wisp_sync::init_trusted_replica(&replica, &root, &unlock()).unwrap());
    let mut repo = Repository::open(&root, &unlock()).unwrap();

    // The first change with inputs_complete raises the format.
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    update(
        &mut repo,
        m,
        AGENT,
        "notes/a.md",
        "A",
        "y\n",
        json!([i]),
        json!({ "inputs_complete": true }),
    )
    .unwrap();
    assert_eq!(format_of(&root), FORMAT_INPUTS_COMPLETE);

    // Sync carries the marker with the records.
    wisp_sync::push(&mut repo, &replica, m, &unlock()).unwrap();
    assert_eq!(format_of(&replica), FORMAT_INPUTS_COMPLETE);
    drop(repo);

    // A format this build does not know is refused with an upgrade message, not reported as damage.
    let path = root.join(".wisp/manifest.toml");
    let mut mf = Manifest::load(&path).unwrap();
    mf.repo.format = FORMAT_INPUTS_COMPLETE + 1;
    mf.save(&path).unwrap();
    let e = Repository::open(&root, &unlock()).err().expect("refused");
    assert_eq!(e.code, ExitCode::Refused);
    assert_eq!(
        e.message,
        "this repository uses a newer format; upgrade wisp"
    );
    assert_eq!(e.details["code"], "format_too_new");
}

#[test]
fn frontmatter_versions_inherit_as_one_unit() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", SRC),
        ("notes/a.md", NOTE),
        ("notes/c.md", NOTE),
    ]);
    let m = main_ref();
    let set_key = |repo: &mut Repository,
                   actor: &str,
                   key: &str,
                   inputs: Value,
                   complete: bool|
     -> ChangeId {
        let r = apply_json(
            repo,
            json!({ "ref": m, "actor": actor, "inputs": inputs, "inputs_complete": complete,
                    "ops": [{ "op": "set_key", "path": "notes/a.md", "key": key, "base_version": "absent", "value": "v" }] }),
        )
        .unwrap();
        r["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["op"] == "set_key")
            .and_then(|c| c["version"].as_str())
            .unwrap()
            .parse()
            .unwrap()
    };
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let f1 = set_key(&mut repo, AGENT, "owner", json!([i]), false);
    // A different key, no inputs: the frontmatter as a whole still inherits.
    let f2 = set_key(&mut repo, HUMAN, "team", json!([]), false);
    // Declared complete and empty: the chain stops, and so does what follows it.
    let f3 = set_key(&mut repo, HUMAN, "tier", json!([]), true);
    let f4 = set_key(&mut repo, HUMAN, "region", json!([]), false);
    // A reader of the whole concept, pinned after f2 but before f3, depends on the frontmatter.
    let v = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert!(versions(&v).contains(&f1.to_string()), "{v:#}");
    let r2 = result_for(&v, f2);
    assert_eq!(r2["kind"], "frontmatter");
    assert_eq!(r2["link"], "inherited");
    assert!(!versions(&v).contains(&f3.to_string()));
    assert!(!versions(&v).contains(&f4.to_string()));
    assert_eq!(result_for(&v, f1)["kind"], "frontmatter");

    let i = json!({ "concept": "notes/a.md", "at": head(&mut repo, m).to_string() });
    let (c1, _) = {
        let r = update(
            &mut repo,
            m,
            AGENT,
            "notes/c.md",
            "A",
            "reads a\n",
            json!([i]),
            json!({}),
        )
        .unwrap();
        (written_version(&r), r)
    };
    // Pinned after f4: the frontmatter head there (f4) is outside the chain, and the section of
    // a.md was never derived from the source, so c does not depend on it.
    let v = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert!(!versions(&v).contains(&c1.to_string()), "{v:#}");
}

#[test]
fn erase_reports_dependents_on_every_ref_grouped_by_ref() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", SRC),
        ("notes/a.md", NOTE),
        ("notes/b.md", NOTE),
        ("notes/c.md", NOTE),
    ]);
    let m = main_ref();
    wisp_engine::query::branch(&mut repo, "feature", None).unwrap();
    wisp_engine::query::branch(&mut repo, "unrelated", None).unwrap();
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let a1 = written_version(
        &update(
            &mut repo,
            m,
            AGENT,
            "notes/a.md",
            "A",
            "a\n",
            json!([i]),
            json!({}),
        )
        .unwrap(),
    );
    let i = pinned(&mut repo, "feature", "sources/src.md", "S");
    let b1 = written_version(
        &update(
            &mut repo,
            "feature",
            AGENT,
            "notes/b.md",
            "A",
            "b\n",
            json!([i]),
            json!({}),
        )
        .unwrap(),
    );
    update(
        &mut repo,
        "unrelated",
        AGENT,
        "notes/c.md",
        "A",
        "c\n",
        json!([]),
        json!({}),
    )
    .unwrap();
    let a = concept_id(&mut repo, m, "notes/a.md");
    let b = concept_id(&mut repo, m, "notes/b.md");

    // impact stays ref-scoped.
    assert_eq!(
        versions(&impact(&mut repo, m, "sources/src.md", 3).unwrap()),
        BTreeSet::from([a1.to_string()])
    );

    let target = EraseTarget::Path("sources/src.md".into());
    let preview =
        wisp_engine::erase::erase(&mut repo, m, &target, Reason::Policy, false, false, HUMAN)
            .unwrap();
    assert!(
        preview["dependents_by_ref"]["feature"].is_array(),
        "{preview:#}"
    );
    let r = wisp_engine::erase::erase(&mut repo, m, &target, Reason::Policy, true, false, HUMAN)
        .unwrap();
    let by_ref = r["dependents_by_ref"].as_object().unwrap();
    let versions_on = |name: &str| -> BTreeSet<String> {
        by_ref[name]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["version"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        versions_on("main"),
        BTreeSet::from([a1.to_string()]),
        "{r:#}"
    );
    assert_eq!(versions_on("feature"), BTreeSet::from([b1.to_string()]));
    assert!(!by_ref.contains_key("unrelated"));
    let mut flagged = vec![a.to_string(), b.to_string()];
    flagged.sort();
    assert_eq!(r["flagged"], json!(flagged));
}

#[test]
fn identical_content_merged_from_both_sides_carries_both_dependency_sets() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/one.md", SRC),
        ("sources/two.md", SRC2),
        ("notes/a.md", NOTE),
        ("notes/c.md", NOTE),
    ]);
    let m = main_ref();
    wisp_engine::query::branch(&mut repo, "feature", None).unwrap();
    // The same bytes written on both sides, each from a different recorded source.
    let i = pinned(&mut repo, m, "sources/one.md", "S");
    let on_main = written_version(
        &update(
            &mut repo,
            m,
            AGENT,
            "notes/a.md",
            "A",
            "same text\n",
            json!([i]),
            json!({}),
        )
        .unwrap(),
    );
    let i = pinned(&mut repo, "feature", "sources/two.md", "S");
    let on_feature = written_version(
        &update(
            &mut repo,
            "feature",
            AGENT,
            "notes/a.md",
            "A",
            "same text\n",
            json!([i]),
            json!({}),
        )
        .unwrap(),
    );
    let merged = wisp_engine::merge::merge_refs(&mut repo, "feature", m, None, false).unwrap();
    assert_ne!(
        merged["status"], "conflicts",
        "identical content merges cleanly: {merged:#}"
    );

    // After the merge: a reader pinned to the merged state, and a later edit with no inputs.
    let i = pinned(&mut repo, m, "notes/a.md", "A");
    let reader = written_version(
        &update(
            &mut repo,
            m,
            AGENT,
            "notes/c.md",
            "A",
            "from a\n",
            json!([i]),
            json!({}),
        )
        .unwrap(),
    );
    let edit = written_version(
        &update(
            &mut repo,
            m,
            HUMAN,
            "notes/a.md",
            "A",
            "same text, edited\n",
            json!([]),
            json!({}),
        )
        .unwrap(),
    );

    for (src, own) in [("sources/one.md", on_main), ("sources/two.md", on_feature)] {
        let v = impact(&mut repo, m, src, 3).unwrap();
        let got = versions(&v);
        assert!(got.contains(&own.to_string()), "{src}: {v:#}");
        assert!(
            got.contains(&reader.to_string()),
            "{src}: the merged state carries this source: {v:#}"
        );
        assert!(
            got.contains(&edit.to_string()),
            "{src}: the next version inherits both sets: {v:#}"
        );
    }
}

//! `wisp impact` (historical mode).
//!
//! Fixture: `wisp-corpus/data/bundles/impact-demo/`
//! Narrative: `wisp-corpus/data/scenarios/playbook/impact-demo/README.md`

mod impact_harness;

use impact_harness::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use wisp_engine::erase::EraseTarget;
use wisp_engine::impact::{impact, EMPTY_MESSAGE};
use wisp_engine::repo::Repository;
use wisp_invariants::{catalog, init_and_record_bundle};
use wisp_model::{ChangeId, ExitCode, Reason};

const OLD: &str = "sources/payments-api-2026-09.md";
const NEW: &str = "sources/payments-api-2026-10.md";
const AUTH: &str = "sources/auth-spec.md";
const RETRY: &str = "notes/retry-policy.md";
const ERRORS: &str = "notes/error-handling.md";
const WEBHOOK: &str = "notes/webhook-setup.md";
const AUTHFLOW: &str = "notes/auth-flow.md";
const DEPLOY: &str = "notes/deploy-checklist.md";

fn demo_repo() -> (tempfile::TempDir, std::path::PathBuf, Repository) {
    let spec = catalog()
        .bundle("impact-demo")
        .expect("impact-demo bundle")
        .clone();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let repo = init_and_record_bundle(&spec, &root, &unlock()).unwrap();
    (dir, root, repo)
}

/// An input pinned at the ref's current head (what an agent passes after `wisp cat --json`).
fn pinned(repo: &mut Repository, ref_name: &str, path: &str, heading: &str) -> Value {
    let at = head(repo, ref_name);
    input(repo, ref_name, path, Some(heading), Some(at))
}

fn results(v: &Value) -> Vec<Value> {
    v["results"].as_array().cloned().unwrap_or_default()
}

fn versions(v: &Value) -> BTreeSet<String> {
    results(v)
        .iter()
        .map(|r| r["version"].as_str().unwrap().to_string())
        .collect()
}

fn result_for(v: &Value, version: ChangeId) -> &Value {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["version"] == json!(version.to_string()))
        .unwrap_or_else(|| panic!("no result for {version} in {v:#}"))
}

fn set(vs: &[ChangeId]) -> BTreeSet<String> {
    vs.iter().map(|v| v.to_string()).collect()
}

fn upd(
    repo: &mut Repository,
    actor: &str,
    path: &str,
    heading: &str,
    content: &str,
    inputs: Value,
) -> (ChangeId, Value) {
    let r = update(
        repo,
        main_ref(),
        actor,
        path,
        heading,
        content,
        inputs,
        json!({}),
    )
    .unwrap();
    (written_version(&r), r)
}

#[test]
fn demo_five_cases() {
    let (_d, _root, mut repo) = demo_repo();
    let m = main_ref();

    // 1. Direct dependency.
    let i = pinned(&mut repo, m, OLD, "Retries");
    let (retry_v1, _) = upd(
        &mut repo,
        AGENT,
        RETRY,
        "Policy",
        "Retry 10 times, no backoff.\n",
        json!([i]),
    );
    // 2. Two-step chain through the affected version.
    let i = pinned(&mut repo, m, RETRY, "Policy");
    let (errors_v1, _) = upd(
        &mut repo,
        AGENT,
        ERRORS,
        "Guide",
        "On 5xx, retry per the retry policy.\n",
        json!([i]),
    );
    // 3. Written from the old doc, then completely rewritten from the corrected doc, then a typo fix.
    let i = pinned(&mut repo, m, OLD, "Webhooks");
    let (wh_v1, _) = upd(
        &mut repo,
        AGENT,
        WEBHOOK,
        "Setup",
        "Accept unsigned payloads.\n",
        json!([i]),
    );
    let i = pinned(&mut repo, m, NEW, "Webhooks");
    let r = update(
        &mut repo,
        m,
        AGENT,
        WEBHOOK,
        "Setup",
        "Verify the HMAC signature on every payload.\n",
        json!([i]),
        json!({ "inputs_complete": true }),
    )
    .unwrap();
    let wh_v2 = written_version(&r);
    let (wh_v3, _) = upd(
        &mut repo,
        HUMAN,
        WEBHOOK,
        "Setup",
        "Verify the HMAC signature on every webhook payload.\n",
        json!([]),
    );
    // 4. Independent source.
    let i = pinned(&mut repo, m, AUTH, "Tokens");
    let (auth_v1, _) = upd(
        &mut repo,
        AGENT,
        AUTHFLOW,
        "Flow",
        "Refresh tokens rotate on use.\n",
        json!([i]),
    );
    // 5. No declared inputs: warned, never refused.
    let (deploy_v1, r) = upd(
        &mut repo,
        AGENT,
        DEPLOY,
        "Checklist",
        "Run migrations, then deploy.\n",
        json!([]),
    );
    assert_eq!(r["warnings"][0]["code"], "no_declared_inputs");

    // The old doc is withdrawn. Retract lists the direct dependents.
    let retry_c = concept_id(&mut repo, m, RETRY);
    let webhook_c = concept_id(&mut repo, m, WEBHOOK);
    let old_c = concept_id(&mut repo, m, OLD);
    let rr = wisp_engine::erase::retract(&mut repo, m, OLD, Reason::SourceWithdrawn, None, HUMAN)
        .unwrap();
    let mut expected = vec![retry_c.to_string(), webhook_c.to_string()];
    expected.sort();
    assert_eq!(rr["flagged"], json!(expected), "{rr}");

    let v = impact(&mut repo, m, OLD, 3).unwrap();
    assert_eq!(v["source"]["state"], "retracted", "{v:#}");
    assert_eq!(v["source"]["concept_id"], json!(old_c.to_string()));
    assert_eq!(v["mode"], "historical");
    assert_eq!(v["truncated"], false);
    assert_eq!(v["message"], Value::Null);
    assert_eq!(versions(&v), set(&[retry_v1, errors_v1, wh_v1]), "{v:#}");
    for not in [wh_v2, wh_v3, auth_v1, deploy_v1] {
        assert!(!versions(&v).contains(&not.to_string()));
    }
    assert!(v["untracked_content_changes"].as_u64().unwrap() >= 1);

    let direct = result_for(&v, retry_v1);
    assert_eq!(direct["depth"], 1);
    assert_eq!(direct["link"], "pinned");
    assert_eq!(direct["actor"], AGENT);
    assert!(direct["at"].is_string());
    let chain = result_for(&v, errors_v1);
    assert_eq!(chain["depth"], 2);
    let via = chain["via"].as_array().unwrap();
    assert_eq!(via.len(), 2, "{chain:#}");
    assert_eq!(via[0]["source"], true);
    assert_eq!(via[0]["link"], "pinned");
    assert!(via[0]["snapshot"].is_string());
    assert!(
        via[0]["version"].is_string(),
        "pinned source hop names the version read"
    );
    assert_eq!(via[1]["version"], json!(retry_v1.to_string()));
    assert_eq!(via[1]["link"], "pinned");
    assert_eq!(via[1]["path"], "notes/retry-policy");

    // The corrected doc's dependents: v2 (declared), v3 (inherited from v2).
    let v = impact(&mut repo, m, NEW, 3).unwrap();
    assert_eq!(versions(&v), set(&[wh_v2, wh_v3]), "{v:#}");
    assert_eq!(result_for(&v, wh_v3)["link"], "inherited");

    // Nothing recorded depends on the deploy checklist.
    let v = impact(&mut repo, m, DEPLOY, 3).unwrap();
    assert!(results(&v).is_empty());
    assert_eq!(v["message"], EMPTY_MESSAGE);
}

#[test]
fn pins_inheritance_and_unversioned_links() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", "---\ntype: Source\n---\n## S\n\nsource\n"),
        ("notes/p.md", "---\ntype: Note\n---\n## Policy\n\nTBD\n"),
        ("notes/q.md", "---\ntype: Note\n---\n## Q\n\nTBD\n"),
        ("notes/r.md", "---\ntype: Note\n---\n## R\n\nTBD\n"),
        ("notes/z.md", "---\ntype: Note\n---\n## Z\n\nTBD\n"),
    ]);
    let m = main_ref();
    // z read p before p was derived from the source: the pin rules z out.
    let early = pinned(&mut repo, m, "notes/p.md", "Policy");
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let (p1, _) = upd(
        &mut repo,
        AGENT,
        "notes/p.md",
        "Policy",
        "from source\n",
        json!([i]),
    );
    let (z1, _) = upd(
        &mut repo,
        AGENT,
        "notes/z.md",
        "Z",
        "from early p\n",
        json!([early]),
    );
    // A typo fix with no inputs inherits p1's dependencies.
    let (p2, _) = upd(
        &mut repo,
        HUMAN,
        "notes/p.md",
        "Policy",
        "from the source\n",
        json!([]),
    );
    // q pinned the typo-fixed version; r named p with no section and no version.
    let i = pinned(&mut repo, m, "notes/p.md", "Policy");
    let (q1, _) = upd(&mut repo, AGENT, "notes/q.md", "Q", "from p\n", json!([i]));
    let (r1, _) = upd(
        &mut repo,
        AGENT,
        "notes/r.md",
        "R",
        "from p\n",
        json!([{ "concept": "notes/p.md" }]),
    );

    let v = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert_eq!(versions(&v), set(&[p1, p2, q1, r1]), "{v:#}");
    assert!(!versions(&v).contains(&z1.to_string()));
    assert_eq!(result_for(&v, p2)["link"], "inherited");
    assert_eq!(
        result_for(&v, p2)["depth"],
        1,
        "inherited hops do not count toward depth"
    );
    let q = result_for(&v, q1);
    assert_eq!(q["depth"], 2);
    let links: Vec<&str> = q["via"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["link"].as_str().unwrap())
        .collect();
    assert_eq!(links, vec!["pinned", "inherited", "pinned"], "{q:#}");
    let r = result_for(&v, r1);
    assert_eq!(r["link"], "unversioned");
    let last = r["via"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["concept_level"], true);
    assert_eq!(v["unversioned_links"], 1);
}

#[test]
fn branch_isolation_and_merge() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", "---\ntype: Source\n---\n## S\n\nsource\n"),
        ("notes/a.md", "---\ntype: Note\n---\n## A\n\nTBD\n"),
        ("notes/b.md", "---\ntype: Note\n---\n## B\n\nTBD\n"),
    ]);
    let m = main_ref();
    wisp_engine::query::branch(&mut repo, "feature", None).unwrap();
    let i = pinned(&mut repo, "feature", "sources/src.md", "S");
    let r = update(
        &mut repo,
        "feature",
        AGENT,
        "notes/a.md",
        "A",
        "from source\n",
        json!([i]),
        json!({}),
    )
    .unwrap();
    let a1 = written_version(&r);

    let on_main = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert!(results(&on_main).is_empty(), "{on_main:#}");
    assert_eq!(on_main["message"], EMPTY_MESSAGE);
    let on_feature = impact(&mut repo, "feature", "sources/src.md", 3).unwrap();
    assert_eq!(versions(&on_feature), set(&[a1]));

    // Diverge main, then merge the branch in: the dependency is now reachable from main.
    upd(
        &mut repo,
        HUMAN,
        "notes/b.md",
        "B",
        "main-only edit\n",
        json!([]),
    );
    wisp_engine::merge::merge_refs(&mut repo, "feature", m, None, false).unwrap();
    let merged = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert_eq!(versions(&merged), set(&[a1]), "{merged:#}");
}

#[test]
fn cycles_terminate_and_dedupe() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", "---\ntype: Source\n---\n## S\n\nsource\n"),
        ("notes/a.md", "---\ntype: Note\n---\n## A\n\nTBD\n"),
        ("notes/b.md", "---\ntype: Note\n---\n## B\n\nTBD\n"),
        ("notes/c.md", "---\ntype: Note\n---\n## C\n\nTBD\n"),
    ]);
    let m = main_ref();
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let (a1, _) = upd(
        &mut repo,
        AGENT,
        "notes/a.md",
        "A",
        "a from src\n",
        json!([i]),
    );
    let i = input(&mut repo, m, "notes/a.md", Some("A"), None);
    let (b1, _) = upd(
        &mut repo,
        AGENT,
        "notes/b.md",
        "B",
        "b from a\n",
        json!([i]),
    );
    let i = input(&mut repo, m, "notes/b.md", Some("B"), None);
    let (a2, _) = upd(
        &mut repo,
        AGENT,
        "notes/a.md",
        "A",
        "a from b\n",
        json!([i]),
    );
    let i = pinned(&mut repo, m, "notes/a.md", "A");
    let (c1, _) = upd(
        &mut repo,
        AGENT,
        "notes/c.md",
        "C",
        "c from a\n",
        json!([i]),
    );

    let v = impact(&mut repo, m, "sources/src.md", 10).unwrap();
    let all: Vec<String> = results(&v)
        .iter()
        .map(|r| r["version"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(all.len(), versions(&v).len(), "each version once: {all:?}");
    assert_eq!(versions(&v), set(&[a1, b1, a2, c1]), "{v:#}");
    assert_eq!(v["truncated"], false);
}

#[test]
fn depth_truncation_is_reported_only_when_dependencies_remain() {
    let files: Vec<(String, String)> = (1..=4)
        .map(|n| {
            (
                format!("notes/n{n}.md"),
                format!("---\ntype: Note\n---\n## N{n}\n\nTBD\n"),
            )
        })
        .chain([(
            "sources/src.md".to_string(),
            "---\ntype: Source\n---\n## S\n\nsource\n".to_string(),
        )])
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (_d, _root, mut repo) = fresh(&refs);
    let m = main_ref();
    let mut prev = ("sources/src.md".to_string(), "S".to_string());
    let mut chain = Vec::new();
    for n in 1..=4 {
        let i = pinned(&mut repo, m, &prev.0, &prev.1);
        let (v, _) = upd(
            &mut repo,
            AGENT,
            &format!("notes/n{n}.md"),
            &format!("N{n}"),
            "derived\n",
            json!([i]),
        );
        chain.push(v);
        prev = (format!("notes/n{n}.md"), format!("N{n}"));
    }
    let v = impact(&mut repo, m, "sources/src.md", 2).unwrap();
    assert_eq!(versions(&v), set(&chain[..2]));
    assert_eq!(v["truncated"], true);
    let v = impact(&mut repo, m, "sources/src.md", 4).unwrap();
    assert_eq!(versions(&v), set(&chain));
    assert_eq!(v["truncated"], false, "exact fit is not truncation");
    let v = impact(&mut repo, m, "sources/src.md", 9).unwrap();
    assert_eq!(v["truncated"], false);
    assert_eq!(
        impact(&mut repo, m, "sources/src.md", 0).unwrap_err().code,
        ExitCode::Usage
    );
}

#[test]
fn erased_source_and_intermediate_trace_by_opaque_id() {
    let (_d, _root, mut repo) = fresh(&[
        (
            "sources/src.md",
            "---\ntype: Source\n---\n## S\n\nsecret source text\n",
        ),
        ("notes/a.md", "---\ntype: Note\n---\n## A\n\nTBD\n"),
        ("notes/b.md", "---\ntype: Note\n---\n## B\n\nTBD\n"),
    ]);
    let m = main_ref();
    let src_c = concept_id(&mut repo, m, "sources/src.md");
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let (a1, _) = upd(
        &mut repo,
        AGENT,
        "notes/a.md",
        "A",
        "a from src\n",
        json!([i]),
    );
    let i = pinned(&mut repo, m, "notes/a.md", "A");
    let (b1, _) = upd(
        &mut repo,
        AGENT,
        "notes/b.md",
        "B",
        "b from a\n",
        json!([i]),
    );
    let (a_sid, _) = section(&mut repo, m, "notes/a.md", "A");

    wisp_engine::erase::erase(
        &mut repo,
        m,
        &EraseTarget::Path("sources/src.md".into()),
        Reason::Policy,
        true,
        false,
        HUMAN,
    )
    .unwrap();
    wisp_engine::erase::erase(
        &mut repo,
        m,
        &EraseTarget::Path(format!("notes/a.md#{a_sid}")),
        Reason::Policy,
        true,
        false,
        HUMAN,
    )
    .unwrap();

    // The path label is gone with the source.
    assert_eq!(
        impact(&mut repo, m, "sources/src.md", 3).unwrap_err().code,
        ExitCode::NotFound
    );
    let v = impact(&mut repo, m, &src_c.to_string(), 3).unwrap();
    assert_eq!(v["source"]["state"], "erased", "{v:#}");
    assert_eq!(v["source"]["path"], Value::Null);
    assert_eq!(versions(&v), set(&[a1, b1]), "{v:#}");
    let b = result_for(&v, b1);
    let via = b["via"].as_array().unwrap();
    assert_eq!(via[0]["path"], Value::Null);
    assert_eq!(via[0]["concept_id"], json!(src_c.to_string()));
    assert_eq!(
        via[1]["section_id"],
        json!(a_sid.to_string()),
        "erased intermediate keeps its opaque id"
    );
    let fsck = wisp_engine::query::fsck(&mut repo).unwrap();
    assert!(fsck["problems"].as_array().unwrap().is_empty(), "{fsck}");
}

#[test]
fn split_and_merge_lineage_carry_dependencies() {
    let (_d, _root, mut repo) = fresh(&[
        ("sources/src.md", "---\ntype: Source\n---\n## S\n\nsource\n"),
        (
            "notes/n.md",
            "---\ntype: Note\n---\n## Policy\n\nTBD\n\n## Other\n\nTBD\n",
        ),
    ]);
    let m = main_ref();
    let i = pinned(&mut repo, m, "sources/src.md", "S");
    let (n1, _) = upd(
        &mut repo,
        AGENT,
        "notes/n.md",
        "Policy",
        "one\n\ntwo\n",
        json!([i]),
    );
    let (policy, _) = section(&mut repo, m, "notes/n.md", "Policy");
    // A human splits the derived section; the new section carries its dependencies.
    let r = apply_json(
        &mut repo,
        json!({ "ref": m, "actor": HUMAN, "ops": [
            { "op": "add_section", "path": "notes/n.md", "after": policy.to_string(),
              "heading": "Details", "content": "two\n", "split_from": policy.to_string() }
        ]}),
    )
    .unwrap();
    let split_child = written_version(&r);
    // Another section absorbs the derived one by merge.
    let (other, ov) = section(&mut repo, m, "notes/n.md", "Other");
    let r = apply_json(
        &mut repo,
        json!({ "ref": m, "actor": HUMAN, "ops": [
            { "op": "update_section", "path": "notes/n.md", "section": other.to_string(),
              "base_version": ov.to_string(), "content": "one\n", "merged_from": [policy.to_string()] }
        ]}),
    )
    .unwrap();
    let merged = written_version(&r);

    let v = impact(&mut repo, m, "sources/src.md", 3).unwrap();
    assert!(versions(&v).contains(&n1.to_string()));
    assert_eq!(result_for(&v, split_child)["link"], "split", "{v:#}");
    assert_eq!(result_for(&v, merged)["link"], "merge", "{v:#}");
}

#[test]
fn impact_is_read_only() {
    let (_d, root, mut repo) = demo_repo();
    let m = main_ref();
    let i = pinned(&mut repo, m, OLD, "Retries");
    upd(
        &mut repo,
        AGENT,
        RETRY,
        "Policy",
        "Retry 10 times.\n",
        json!([i]),
    );
    let snapshot = |root: &std::path::Path| -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for e in walkdir::WalkDir::new(root).sort_by_file_name() {
            let e = e.unwrap();
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            let tracked = !rel.starts_with(".wisp") || rel == ".wisp/checkout.idx";
            if e.file_type().is_file() && tracked {
                out.push((rel, std::fs::read(e.path()).unwrap()));
            }
        }
        out
    };
    let before_tree = snapshot(&root);
    let seq = wisp_store::Ledger::max_seq(repo.ledger.conn()).unwrap();
    let refs = wisp_store::Ledger::refs(repo.ledger.conn()).unwrap();
    for spec in [OLD, RETRY, ERRORS] {
        impact(&mut repo, m, spec, 3).unwrap();
    }
    assert_eq!(
        wisp_store::Ledger::max_seq(repo.ledger.conn()).unwrap(),
        seq
    );
    assert_eq!(wisp_store::Ledger::refs(repo.ledger.conn()).unwrap(), refs);
    assert_eq!(snapshot(&root), before_tree);
}

/// Rough scaling probe (debug builds are slow; run with `--ignored --nocapture`).
#[test]
#[ignore]
fn impact_scaling_probe() {
    let notes = 40usize;
    let rounds = 25usize;
    let mut files: Vec<(String, String)> = (0..notes)
        .map(|n| {
            (
                format!("notes/n{n}.md"),
                format!("---\ntype: Note\n---\n## N{n}\n\nTBD\n"),
            )
        })
        .collect();
    files.push((
        "sources/src.md".into(),
        "---\ntype: Source\n---\n## S\n\nsource\n".into(),
    ));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (_d, _root, mut repo) = fresh(&refs);
    let m = main_ref();
    let t = std::time::Instant::now();
    for r in 0..rounds {
        for n in 0..notes {
            // Each note pins its predecessor (the source for n0), at the current head.
            let (p, h) = if n == 0 {
                ("sources/src.md".to_string(), "S".to_string())
            } else {
                (format!("notes/n{}.md", n - 1), format!("N{}", n - 1))
            };
            let i = if r % 3 == 2 {
                input(&mut repo, m, &p, Some(&h), None)
            } else {
                pinned(&mut repo, m, &p, &h)
            };
            upd(
                &mut repo,
                AGENT,
                &format!("notes/n{n}.md"),
                &format!("N{n}"),
                &format!("r{r}\n"),
                json!([i]),
            );
        }
    }
    let writes = notes * rounds;
    eprintln!("built {writes} writes in {:?}", t.elapsed());
    for depth in [3usize, 10, 50] {
        let t = std::time::Instant::now();
        let v = impact(&mut repo, m, "sources/src.md", depth).unwrap();
        eprintln!(
            "depth {depth}: {} results, truncated {}, {:?}",
            results(&v).len(),
            v["truncated"],
            t.elapsed()
        );
    }
}

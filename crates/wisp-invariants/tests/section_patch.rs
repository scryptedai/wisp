//! `append_section` / `prepend_section` (logging patches, no version precondition).

mod spy_harness;

use spy_harness::*;

#[test]
fn append_and_prepend_stitch_on_same_section() {
    let cat = wisp_invariants::catalog();
    let spec = cat.bundle("battery-error-three").expect("bundle");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let (sec, _) = section_at(&mut repo, "notes/swarm-metrics", "Frontier");
    let base = section_body(&mut repo, "notes/swarm-metrics", "Frontier");

    patch_section(
        &mut repo,
        "human:calibrator/1",
        "notes/swarm-metrics",
        sec,
        "append_section",
        "line A\n",
        "append A",
    )
    .unwrap();
    patch_section(
        &mut repo,
        "human:calibrator/1",
        "notes/swarm-metrics",
        sec,
        "prepend_section",
        "line Z\n",
        "prepend Z",
    )
    .unwrap();

    let body = section_body(&mut repo, "notes/swarm-metrics", "Frontier");
    assert!(body.starts_with("line Z\n"));
    assert!(body.contains(&base));
    assert!(body.ends_with("line A\n"));
}

#[test]
fn dual_blind_append_same_section_no_conflict() {
    let cat = wisp_invariants::catalog();
    let spec = cat.bundle("battery-error-three").expect("bundle");
    let (_dir, _root, mut repo) = fresh_spy_repo(spec);
    let (sec, _) = section_at(&mut repo, "notes/swarm-metrics", "Frontier");

    patch_section(
        &mut repo,
        "loq-explorer-1/1",
        "notes/swarm-metrics",
        sec,
        "append_section",
        "explorer-1 tick\n",
        "a",
    )
    .unwrap();
    patch_section(
        &mut repo,
        "loq-explorer-2/1",
        "notes/swarm-metrics",
        sec,
        "append_section",
        "explorer-2 tick\n",
        "b",
    )
    .unwrap();

    let body = section_body(&mut repo, "notes/swarm-metrics", "Frontier");
    assert!(body.contains("explorer-1 tick"));
    assert!(body.contains("explorer-2 tick"));
    assert!(wisp_engine::query::fsck(&mut repo).unwrap()["ok"]
        .as_bool()
        .unwrap());
}

# Ref merge scenarios (§11.5)

Integration contracts: `wisp-invariants/tests/merge_playbook.rs` calls `wisp_engine::merge::merge_refs`.

| Scenario | Bundle | Expected merge result |
|----------|--------|------------------------|
| `merge.disjoint-sections` | `warehouse` | `fast_forward` when only the branch commits; `merged` with disjoint replay when main and branch each touch different sections |
| `merge.same-section-conflict` | `warehouse` | Exit **3** Conflict — `details.conflicts.type` is `merge_conflict` with `scope_conflict` rows |

Each folder has `scenario.toml` (catalog metadata). Steps are encoded in the playbook test, not as data-driven scripts.

**Known engine gap:** after a divergent `merged` replay, `main`’s ref head is set to `source_head`, so `into`-only commits drop off the ref’s parent chain while the hot tier holds the combined state — `wisp fsck` reports `hot_mismatch` on `main`.

# Ref merge scenarios (§11.5)

Integration contracts live in **`wisp-invariants/tests/merge_playbook.rs`** (`wisp_engine::merge::merge_refs`). Narrative and tables: **`docs/MERGE.md`**.

## Corpus scenarios (metadata in `scenario.toml`)

| Scenario id | Bundle | Playbook test | Expected |
|-------------|--------|---------------|----------|
| `merge.disjoint-sections` | `warehouse` | `merge_disjoint_sections_fast_forward_and_replay` | `fast_forward` when only the branch commits; `merged` when main and branch touch disjoint sections; `fsck` + `hot_check` on `main` |
| `merge.same-section-conflict` | `warehouse` | `merge_same_section_conflict_contract` | Exit **3** Conflict; `--resolutions` with `take: theirs` completes merge |

Steps are encoded in Rust, not as data-driven scripts in each folder.

## Additional playbook tests (no `scenario.toml`)

| Test | Intent |
|------|--------|
| `merge_nested_second_divergence_hot_check` | Merge once, diverge again, resolve conflict; nested history + `hot_check` |
| `merge_retract_survives_unrelated_branch_commit` | Branch retracts a concept then main diverges on another concept; merge replays retract |

## Rule battery (separate runner)

Per-row §11.5 rules: **`contrib/merge-rules/cases.toml`** and **`cargo test -p wisp-invariants --test merge_rules`**. Includes stacked branch edits + resolution (`rule_resolution_skips_stacked_branch_edits`).

## Hot tier after merge

Divergent merges rebuild hot state with **layered changeset order** and honor **`merge-skip.json`**. Playbook and rule tests assert **`wisp fsck`** and **`wisp hot-check`** on `main` after merge where applicable.

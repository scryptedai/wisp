# Merge rules battery (§11.5)

Normative rule rows live in **`wisp_merge::RULES`**. This directory lists one **integration case per row** (including record-time duplicate path refusal).

**Runner:** `cargo test -p wisp-invariants --test merge_rules`

**Also run:** `cargo test -p wisp-invariants --test merge_playbook` for scenario-tagged flows, nested merge, retract replay, and resolution contracts (see `wisp-corpus/data/scenarios/merge/README.md` and `docs/MERGE.md`).

Edit **`cases.toml` only** when adding rule probes; implement the matching `rule_*` test in `merge_rules.rs`. Extra tests that are not one-to-one with a `RULES` row (e.g. stacked edits + `--resolutions`) belong in `merge_rules.rs` or `merge_playbook.rs` with a short comment pointing here or at `MERGE.md`.

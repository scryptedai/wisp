# Merge rules battery (§11.5)

Normative rule rows live in `wisp_merge::RULES`. This directory lists one **integration case per row** (including record-time duplicate path refusal).

**Runner:** `cargo test -p wisp-invariants --test merge_rules`

Edit **`cases.toml` only** when adding probes; implement the matching `rule_*` test in `merge_rules.rs`.

# Ref merge rules (§11.5)

This table matches `wisp merge` and the `merge_playbook` / corpus scenarios. Git users can map outcomes to merge-ort style conflicts.

| Situation | Outcome | Resolution |
|-----------|---------|--------------|
| `into` is ancestor of `source` | **Fast-forward** `into` to `source` | Automatic |
| `source` is ancestor of `into` | **Already up to date** | No-op |
| No common ancestor | Error | — |
| Divergent changes touch **disjoint** scopes | **Merged** replay (two-parent merge node) | Automatic |
| Same scope key, different change ids (same commitment class) | **merge_conflict** | `wisp merge --resolutions` (`take`: `ours` \| `theirs`) or abort |
| Same section, stale `--base` on headless write | **section_conflict** (exit 3) | Resubmit with `--base theirs.version` |
| Erased scope on one branch, content change on the other | **Merged**; erase wins (losing change skipped, reported as `erase_wins`) | Automatic |
| Erased scope at commit time (single ref) | **scope_erased** / refused | Fix branch before merge |
| Divergent **concept path** moves (`wisp mv`) | **merge_conflict** | `--resolutions` or pick one path before merge |
| **Section order** (each side adds sections) | **Merged**; layered replay (`base` → `into` → `source`) implements “ours then insert source-only sections after their anchor” | Automatic |
| Two concepts at the same path | **path_conflict** at `record` / `create_concept` | Not a merge-time case |
| After conflict, working-tree fix | Record on `into`, then | `wisp record --resolve` clears `.wisp/merge.json`; finish with `--resolutions` |
| Resolved merge with skipped changes | `cache/refs/<ref>/merge-skip.json` | Hot rebuild/check honor skip list |

**Rule battery:** `contrib/merge-rules/cases.toml` and `cargo test -p wisp-invariants --test merge_rules` — one integration case per row in `wisp_merge::RULES`.

**§7.5 section identity:** incremental `wisp record` uses `wisp-match` (see `DECISIONS.md` §7.5). Ref merge does not re-run the full §7.5 matcher table; it keys conflicts on ledger scope targets and change commitments.

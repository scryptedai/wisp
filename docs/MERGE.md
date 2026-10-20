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
| Erased scope in one branch | Policy / scope_erased | Refused at commit or per policy |
| After conflict, working-tree fix | Record on `into`, then | `wisp record --resolve` clears `.wisp/merge.json`; finish with `--resolutions` |
| Resolved merge with skipped changes | `cache/refs/<ref>/merge-skip.json` | Hot rebuild/check honor skip list |

**Not yet full spec parity:** section order / erase-wins / full §7.5 matcher table during ref merge (see `DECISIONS.md`).

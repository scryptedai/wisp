# Ref merge rules (§11.5)

This document matches `wisp merge` (`wisp_merge::merge` → `wisp_engine::merge::merge_refs`), the **`merge_playbook`** and **`merge_rules`** integration tests, and corpus scenario metadata under `wisp-corpus/data/scenarios/merge/`. Git users can map outcomes to merge-ort style conflicts.

## Outcome table

| Situation | Outcome | Resolution |
|-----------|---------|--------------|
| `into` is ancestor of `source` | **Fast-forward** `into` to `source` | Automatic |
| `source` is ancestor of `into` | **Already up to date** | No-op |
| No common ancestor | Error | — |
| Divergent changes touch **disjoint** scopes | **Merged** (two-parent merge node + replay) | Automatic |
| Same scope key, **same** payload commitment | **Merged** (no conflict) | Automatic |
| Same scope key, **different** commitments | **merge_conflict** | `wisp merge --resolutions` or abort |
| Same section, stale `--base` on headless write | **section_conflict** (exit 3) | Resubmit with `--base theirs.version` |
| Erased scope on one branch, content change on the other | **Merged**; **erase wins** (losing changes skipped; `erase_wins` in JSON) | Automatic |
| Erased scope at commit time (single ref) | **scope_erased** / refused | Fix branch before merge |
| Divergent **concept path** moves (`wisp mv`) | **merge_conflict** | `--resolutions` or pick one path before merge |
| **Section order** (each side adds sections) | **Merged**; layered replay (`base` → `into` → `source`) | Automatic |
| Retract on one branch, section edit on same concept on the other | **merge_conflict** | `--resolutions` or abort |
| Two concepts at the same path | **path_conflict** at `record` / `create_concept` | Not a merge-time case |
| After conflict, working-tree fix | Record on `into`, then | `wisp record --resolve` clears `.wisp/merge.json`; finish with `--resolutions` |
| Resolved merge or erase-wins with skipped changes | `cache/refs/<ref>/merge-skip.json` | Hot rebuild / `hot_check` honor skip list |

## How merge replay works

1. **Analyze** changes on each ref since the merge base (`analyze_divergence`): scope keys, tip change ids per key, optional **change chains** per side, erase-wins, retract-vs-content.
2. **Conflicts** → write `.wisp/merge.json`; return exit **3** unless `--resolutions` is provided.
3. **Clean or resolved merge** → record a **two-parent** merge changeset; persist **skip** change ids when needed; rebuild hot tier with **`ordered_changeset_ids`**:
   - **Merge head:** `closure(base)` + `changesets_since(into, base)` + `changesets_since(source, base)` + merge node.
   - **Linear head after a merge:** closure by seq, expanding any nested two-parent nodes the same way.
   - **`changesets_since`** follows `based_on` but **expands** two-parent changesets so both parent histories are included (nested merges and post-merge linear commits).

Skip metadata is stored at `cache/refs/<ref>/merge-skip.json` with the merge head id. **`hot_check`** and rebuild walk **`based_on`** from the current ref head to find the nearest skip file, so skips still apply after later linear commits on `into`.

Durable skip lists also live in the ledger table `merge_replay_skips` (per merge changeset). The JSON under `cache/` is optional compatibility; hot rebuild consults the ledger first.

### Cache lifecycle (all platforms; required on Windows)

Everything under `.wisp/cache/` is **rebuildable** (hot SQLite, merge-skip JSON, embedding caches). Do not delete or rename paths there while a [`Repository`](../../crates/wisp-engine/src/repo.rs) still has hot tiers open — SQLite keeps `hot.db` locked. On Windows, `remove_dir_all` fails with “file in use”; Unix often allows it and hides the bug.

- **`Repository::close_all_hots`** — drop in-process hot handles before touching individual cache files.
- **`wisp_engine::cache::reopen_without_cache`** — test/tool pattern: drop repo → remove cache → reopen (cold start).
- Production code should prefer **ledger-backed** state (merge skips, hot rebuild) over assuming cache files can be edited in place.

## Conflict and resolution JSON

On conflict, `details` includes:

```json
{
  "type": "merge_conflict",
  "base": "<changeset>",
  "conflicts": [ … ],
  "state": ".wisp/merge.json"
}
```

Each `scope_conflict` row includes:

| Field | Meaning |
|-------|---------|
| `scope` | Debug string for the `ScopeKey` |
| `ours` | Tip change id on `into` for this key |
| `theirs` | Tip change id on `source` for this key |
| `into_changes` | All change ids on `into` for this key since base (oldest first) |
| `source_changes` | All change ids on `source` for this key since base |

`--resolutions` file:

```json
{
  "resolutions": [
    { "ours": "<id>", "theirs": "<id>", "take": "ours" }
  ]
}
```

- **`take: ours`** — skip every id in **`source_changes`** for that row (not only `theirs`), so stacked branch edits on the same scope do not replay.
- **`take: theirs`** — skip every id in **`into_changes`**.

Rows without change chains (e.g. some retract-vs-content conflicts) still skip the single `ours` / `theirs` id.

Successful merge JSON may include **`erase_wins`** (array of rule reports). Resolved merges that also had erase-wins merge erase skips into the skip set and include **`erase_wins`** in the response.

## Rule batteries and tests

| Runner | What it covers |
|--------|----------------|
| `cargo test -p wisp-invariants --test merge_rules` | One case per row in **`wisp_merge::RULES`** (`contrib/merge-rules/cases.toml`) plus **`rule_resolution_skips_stacked_branch_edits`** (resolution + full source chain skip + post-merge `hot_check`) |
| `cargo test -p wisp-invariants --test merge_playbook` | Corpus-tagged scenarios (`merge.disjoint-sections`, `merge.same-section-conflict`) plus nested second merge, retract replay after unrelated divergent commit, resolution + `fsck` / `hot_check` |

Edit **`contrib/merge-rules/cases.toml` only** when adding rule probes; add matching `rule_*` tests in `merge_rules.rs`. Playbook-only contracts live in `merge_playbook.rs` (see `wisp-corpus/data/scenarios/merge/README.md`).

## §7.5 section identity vs merge

Incremental **`wisp record`** uses **`wisp-match`** for section identity (see `DECISIONS.md` §7.5). **Ref merge does not re-run** the full §7.5 matcher table. Merge compares **ledger scope targets** and **payload commitments** on divergent tips (and replays changesets in layered order). Ambiguous file identity is resolved at record time, not at merge time.

## Related code

| Area | Location |
|------|----------|
| Merge entry | `crates/wisp-engine/src/merge/mod.rs` (`merge_refs`) |
| Divergence / erase-wins | `crates/wisp-engine/src/merge/detect.rs` |
| Replay order | `crates/wisp-engine/src/merge/replay.rs` |
| Rule table facade | `crates/wisp-merge/src/lib.rs` (`RULES`, `merge`) |
| CLI | `wisp merge <source> <into> [--resolutions file] [--abort]` |

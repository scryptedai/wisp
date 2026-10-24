# Impact tracing (`wisp impact`)

Read-only forward query: **which recorded changes depend on this source** (directly or transitively), scoped to a ref and depth limit. Pairs with [`wisp why`](./SEARCH.md) (backward). Proposal narrative: Andy's *wisp impact* doc; fixture walkthrough: `wisp-corpus/data/scenarios/playbook/impact-demo/README.md`.

**v1 ships historical mode only.** `current` mode (visible heads + complete replacement) is not implemented; do not treat results as "safe to show an agent" without reading `mode` and `limitations` in JSON.

## CLI

```sh
wisp impact <path>[#heading] [--ref main] [--depth N] [--json]
wisp impact <concept-or-section-id> …   # opaque ids after erasure
```

Record dependencies when writing:

```sh
wisp apply …   # batch or per-op `inputs` — see docs/APPLY.md
wisp record --input path#section@<changeset> …
wisp apply … --inputs-complete          # declares full replacement of dependency set (raises repo.format)
```

Agents should pin reads: use `at` from `wisp cat --json` / read assertions, not the commit-time head.

## Test design (CI)

| Area | Tests | What they prove |
|------|--------|-----------------|
| Input pins & warnings | `wisp-invariants/tests/input_capture.rs` | `@changeset` validation, per-op `inputs`, `inputs_complete` + format migration, `no_declared_inputs` warning (never blocks) |
| Demo five cases | `impact.rs` `demo_five_cases` | Retry (d1), errors (d2), webhook historical-only (replacement), auth independent, deploy untracked |
| Pins / inheritance | `pins_inheritance_and_unversioned_links` | Unversioned vs pinned edges; conservative inheritance without `inputs_complete` |
| Branch scope | `branch_isolation_and_merge` | Impact respects ref history, not other branches |
| Cycles | `cycles_terminate_and_dedupe` | Traversal terminates; no duplicate section versions in results |
| Depth | `depth_truncation_is_reported_only_when_dependencies_remain` | `truncated` only when outgoing deps were cut |
| Erasure | `erased_source_and_intermediate_trace_by_opaque_id` | Opaque concept id still resolves; labels may be absent |
| Lineage | `split_and_merge_lineage_carry_dependencies` | Split/merge does not silently drop recorded deps |
| Safety | `impact_is_read_only` | No ledger writes from `impact`; harness flushes materialize after headless apply so disk/`checkout.idx` stay stable during the read-only check |
| Scale | `impact_scaling_probe` (ignored) | Local timing probe; not CI-gated |
| Follow-ups | `impact_followups.rs` | Frontmatter units, identical-merge union, erase-on-ref behavior |
| `wisp why` | `why_scope.rs` | Backward walk scoped to selected ref |
| Retract | `retract_dependents.rs` | `wisp retract` lists recorded dependents |

Corpus bundle: `impact-demo` in `wisp-corpus/catalog.toml` (record, validate, fsck in corpus harness).

## Trust and format

- **Stdio / HTTP sync** do not change impact metadata; see [TRUST-BOUNDARY.md](./TRUST-BOUNDARY.md).
- **`repo.format` 2** is set when a repo first records `inputs_complete`. Builds that know the format refuse older readers with `format_too_new`. **wisp 0.1.0 does not read `repo.format`** — upgrade all readers on a shared store before using `--inputs-complete` (see `CHANGELOG.md`).

## Agent harness (recommended)

1. Read with `wisp cat --json`; capture `head` / section snapshot for each source section.
2. Apply or record with `inputs` including `at`; use `--inputs-complete` only when rewriting from a new source set.
3. On retract/erase of a source: `wisp impact <source> --json` → re-read listed dependents in priority order.
4. Gate (optional): warn on `no_declared_inputs` for non-human actors; do not use impact results as pack filters until `current` mode exists.

# Backward tracing (`wisp why`)

Read-only query: **how did this section or concept get here** — content history, actors, origins, and the **input graph** backward from a target.

Pairs with forward tracing in [IMPACT.md](./IMPACT.md). Retrieval and search tiers are documented in [SEARCH.md](./SEARCH.md); `why` is provenance, not ranking.

## CLI

```sh
wisp why <path>[#heading|#section-id] [--depth N] [--ref main] [--json]
```

- **`--depth`** — Hops along declared inputs (inherited versions do not count as hops).
- **`--ref`** — History is scoped to that ref (not the whole ledger).

Use `wisp cat --json` for the `version` / change id agents should pin in `--input` and `--base`.

## Tests

Ref-scoped walks: `wisp-invariants/tests/why_scope.rs`.

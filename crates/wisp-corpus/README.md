# wisp OKF test corpus

Fixture bundles, single-file concepts, and matcher scenarios indexed by [`catalog.toml`](catalog.toml). This is the **test-driven** backbone for finishing wisp: add data first, then wire or extend tests in `wisp-invariants` (and crate unit tests) until they pass.

## Layout

```
data/
  bundles/<id>/          # Full OKF trees (copy → wisp init → record)
  concepts/<id>.md       # Parse-only single files
  scenarios/match/<id>/  # scenario.toml + optional input.md
```

## Adding a bundle

1. Create `data/bundles/<id>/` with OKF markdown (no `wisp_id` or section anchors unless the test needs them).
2. Add a `[[bundle]]` entry to `catalog.toml` with `invariants`, `profile`, and flags:
   - `record = true` — run init + record in integration tests
   - `lossless_strip = true` — after record, checkout with strip anchors; compare to pre-record snapshot via `strip_managed_and_anchors`
3. Run `cargo test -p wisp-invariants`.

## Adding a matcher scenario

1. Create `data/scenarios/match/<name>/scenario.toml` (see existing examples).
2. Add `[[scenario]]` to the catalog.
3. `cargo test -p wisp-invariants corpus_match_scenarios`.

## Domain playbook: SPY options

Bundle **`spy-options`**: strategy, signal rubric, observations (pre-CPI whisper + CPI print), trade log, position limits. Integration narrative in **`tests/spy_options_playbook.rs`** — parallel agents, section conflict + resubmit, section reorder, retract superseded observation, dirty human thesis + agent journal (I19).

## Adversarial fixtures

- **`adversarial-suite`** — broad torture (CRLF, unicode, fences, stray anchors, link soup). `lossless_strip = false` because some YAML list indent styles still reformat on record (documented gap).
- **`adversarial-lossless`** — strict I3 strip round-trip subset.
- **`identity-gauntlet`** — pre-seeded section anchors; used by `tests/adversarial.rs` for repair/reorder/idempotence.
- **`tests/adversarial.rs`** — tampering `index.md`, `wisp_id` strip repair, section reorder.

## Invariant tags

Tags in `catalog.toml` are documentation for humans and filters for future test runners. They map to §17 IDs (e.g. `I3`, `I8`, `I12`).

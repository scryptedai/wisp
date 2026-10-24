# §17 invariant coverage (Community Release 0.1.0)

OKF store **§17** defines invariant IDs **I1–I25**. This repo does not ship the full OKF spec text; this table is the **maintained manifest** of what **CI and tests** exercise versus what is **deferred** or **partial** in 0.1.0.

**Commands:** `cargo test --workspace` (engine + store unit tests), `cargo test -p wisp-invariants` (corpus + integration). See [README.md](../README.md) for air-gap / offline test variants.

| ID | Summary (short) | Coverage in this repo |
|----|-----------------|------------------------|
| I1 | Ledger log append-only | `wisp-store` `i1_ledger_is_append_only`; insert-only triggers in `ledger.rs` |
| I2 | Hot rebuild deterministic | `wisp-engine` `i2_i13_hot_rebuild_is_deterministic` |
| I3 | Lossless parse / strip-anchors round-trip | Engine test; `wisp-invariants` corpus + `adversarial-lossless` bundle |
| I4 | Erasure removes ciphertext / canaries | `i4_i5_i21_i22_erase_section_canaries_and_integrity` |
| I5 | Integrity chain after erasure | Same erase integration test |
| I6 | *(OKF §17 — not mapped in 0.1 tests)* | **Deferred** — add corpus + test when spec clause is wired |
| I7 | Section identity on rename / rewrite | Engine + `adversarial.rs`; corpus bundles tagged `I7` |
| I8 | Bundle validate / conformance | Corpus `catalog.toml` tags; `corpus.rs` record → validate → fsck pipeline |
| I9 | *(not mapped)* | **Deferred** |
| I10 | *(not mapped)* | **Deferred** |
| I11 | *(not mapped)* | **Deferred** |
| I12 | Default write policies (Computation) | `i4_…` policy test; `computation-attested` corpus |
| I13 | Hot tier pure function of ledger | `i2_i13_…`; `hot check` / logical dump in `query.rs` |
| I14 | Erase completes (no pending shred on reopen) | `i4_i5_i21_i22_…` asserts `scopes_erased_not_shredded` empty after reopen |
| I15 | *(not mapped)* | **Deferred** |
| I16 | Concurrent disjoint headless writers | `i16_i17_concurrent_headless_writers_disjoint_sections` |
| I17 | Concurrent writers (paired with I16) | Same test |
| I18 | *(not mapped)* | **Deferred** |
| I19 | Headless write preserves unrecorded edits | `i19_headless_commit_preserves_unrecorded_edits`; spy playbook |
| I20 | *(not mapped)* | **Deferred** |
| I21 | Erased content absent from store | Erase canary test |
| I22 | Re-anchor after erase mints new id | Erase canary test |
| I23 | Idempotent record (no duplicate change) | Engine record idempotence assertion |
| I24 | Actor erase | `erase_whole_concept_and_group_and_actor` |
| I25 | Staging flush on open | Engine `i25` staging test |

## Beyond single-ID tests

| Area | Tests |
|------|--------|
| §11.5 merge | `merge_playbook`, `merge_rules`, `contrib/merge-rules/cases.toml` |
| §11.6 sync | `sync_deletions`, `sync_roundtrip` |
| Search IR | `search_battery`, `search_local_only`; semantic smoke gated on `semantic` feature |
| Headless writers (exit 3, patches) | `section_patch.rs`; `battery_error_three` + `contrib/battery-error-three/cases.toml` (manual refresh); `battery_exit3_adversarial` + `contrib/battery-exit3-adversarial/cases.toml` (`apply_with_resolve`); CI `stress` job (`battery_exit3_adversarial_heavy`, `stress_many_disjoint_writers`) |
| `apply_with_resolve` | `wisp-engine` `i16_i17_concurrent_headless_writers_disjoint_sections` (resolve tail); adversarial battery phases |
| Async materialize (hot vs disk) | `wisp-invariants/materialize_barrier.rs`; engine tests use `flush_materialize` via `sync_disk`; `record` drains in `wisp-engine/src/record.rs` |
| Erase recovery | `wisp-engine` `recover_after_*`; `erase_cause_for_pending_picks_latest_erase_changeset` in `erase.rs` |
| Impact / inputs | `impact.rs`, `input_capture.rs`, `impact_followups.rs`; harnesses call `flush_materialize` after headless apply when disk or erase preflight matters |

When adding behavior, extend **corpus fixtures first**, then **wisp-invariants**, and update this table in the same PR.

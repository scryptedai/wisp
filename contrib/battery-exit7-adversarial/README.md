# Adversarial exit-7 battery

Stress **write queue timeout (exit 7)** under multi-writer headless load — the failure mode from five parallel CLI explorers on one `main` ref. Complements [`battery-exit3-adversarial`](../battery-exit3-adversarial/README.md) (exit 3) and [`concurrency_stress`](../../crates/wisp-invariants/tests/concurrency_stress.rs).

## Phases

| Phase | Intent |
|-------|--------|
| **E1-disjoint-engine-retry** | N writers, disjoint arena sections; only in-engine `commit.retries` |
| **E2-five-explorer-raw** | Report-shaped burst; **no** client exit-7 retry — surfaces saturation |
| **E3-disjoint-client-retry** | Same as E2 with client backoff on exit 7 |
| **E4-batch-vs-single** | Compare wall time: one `apply` batch vs N single-op commits |
| **E5-profile-sequential** | `wisp_engine::timing` phase table (steady state, no contention) |

Edit probes in **`cases.toml`** only.

## Run

```sh
cargo test -p wisp-invariants battery_exit7_adversarial -- --nocapture
```

Heavy (more writers / ops):

```sh
WISP_EXIT7_BATTERY=1 WISP_EXIT7_BATTERY_WRITERS=16 \
  cargo test -p wisp-invariants battery_exit7_adversarial_heavy -- --ignored --nocapture
```

Stdout prints `wall_ms`, per-step `exit7` hits, success counts, and (E5) aggregated commit/apply phases.

Contract: [`docs/AGENT-WRITERS.md`](../../docs/AGENT-WRITERS.md) (exit 7 row).

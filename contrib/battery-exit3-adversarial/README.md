# Adversarial exit-3 battery

Stress **section_conflict (exit 3)** as a **normal, cheap resolution path** for enterprise swarms — not a fleet outage. Complements [`battery-error-three`](../battery-error-three/README.md) (timing tree) and [`concurrency_stress`](../../crates/wisp-invariants/tests/concurrency_stress.rs) (disjoint writers, exit 7).

## Phases

| Phase | Intent |
|-------|--------|
| **A1-stale-stampede** | N writers commit `update_section` with the **same stale** `base_version` → **100% exit 3**, no successes |
| **A2-parallel-resolve** | Same setup, then each writer uses **refresh-on-3** until all N commits land |
| **B1-disjoint-parallel** | N writers on **S0..S{N-1}** with fresh bases → **0 exit 3** |
| **C1-append-stale-update** | `append_section` then stale `update_section` → 3, then resolve |
| **C2-stale-then-blind-append** | Stale update fails; `append_section` still succeeds |
| **D1-retry-budget** | Waves of stale writes; bounded resolver; all succeed within `max_retries` |
| **D2-poison-neighbor** | One client **never** refreshes base; disjoint good writers still commit |

Edit probes in **`cases.toml`** only.

## Run

```sh
cargo test -p wisp-invariants battery_exit3_adversarial -- --nocapture
```

Heavy (more writers):

```sh
WISP_EXIT3_BATTERY=1 WISP_EXIT3_BATTERY_WRITERS=32 \
  cargo test -p wisp-invariants battery_exit3_adversarial_heavy -- --ignored --nocapture
```

Stdout prints `exit3_ms`, `resolve_ms`, `exit7_count`, and per-phase assertions.

## Resolver contract (reference for agents)

Full guide: [`docs/AGENT-WRITERS.md`](../../docs/AGENT-WRITERS.md).

On exit 3: read `details[0].theirs.version` (or refresh section from hot / `wisp cat --json`), set `base_version`, retry. On exit 7: backoff per `commit.retries`. Never treat exit 3 as process-fatal.

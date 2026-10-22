# Concurrency stress (evaluation follow-up)

See **[docs/CONCURRENCY.md](../../docs/CONCURRENCY.md)** for the concurrency model (SQLite ledger, exit 3/7, `worktree.lock`).

The engine ships an **8 writers × 5 updates** disjoint-section test in `crates/wisp-engine/src/tests.rs` (`i16_i17_concurrent_headless_writers_disjoint_sections`).

For larger runs (e.g. **50 writers**), use the optional invariant test:

```sh
WISP_STRESS=1 WISP_STRESS_WRITERS=50 cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers
```

Tune `WISP_STRESS_WRITERS` and ops-per-writer in `crates/wisp-invariants/tests/concurrency_stress.rs`. This is not part of default CI (ignored unless `WISP_STRESS=1`).

## Sample results (disjoint sections)

Reproduce (offline-friendly, no semantic feature):

```sh
CARGO_TARGET_DIR=./target WISP_STRESS=1 WISP_STRESS_WRITERS=50 \
  cargo test -p wisp-invariants --no-default-features -- \
  --ignored stress_many_disjoint_writers
```

| Environment | Writers | Ops/writer | Outcome |
|-------------|---------|------------|---------|
| macOS 26.2 (Darwin 25.2.0), Apple M3, debug build | 50 | 3 | **Pass** (2026-10-21) — `fsck` clean; wall **12.3–21.2s** real over 3 runs (`/usr/bin/time`); exit **7** retried with hot base refresh in the test harness |
| Design target | 50 | 3 | Each writer owns one disjoint section on `main`; production clients should retry exit 7 per manifest `commit.retries` |
| Default CI | 8 | 5 | `i16_i17_concurrent_headless_writers_disjoint_sections` in `wisp-engine` |

Contention rises when many writers share the **same ref** and commit in bursts; use branches/worktrees for parallel feature work.

# Concurrency stress (evaluation follow-up)

See **[docs/CONCURRENCY.md](../../docs/CONCURRENCY.md)** for the concurrency model (SQLite ledger, exit 3/7, `worktree.lock`).

The engine ships an **8 writers × 5 updates** disjoint-section test in `crates/wisp-engine/src/tests.rs` (`i16_i17_concurrent_headless_writers_disjoint_sections`).

For larger runs (e.g. **50 writers**), use the optional invariant test:

```sh
WISP_STRESS=1 WISP_STRESS_WRITERS=50 cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers
```

Tune `WISP_STRESS_WRITERS` and ops-per-writer in `crates/wisp-invariants/tests/concurrency_stress.rs`. This is not part of default CI (ignored unless `WISP_STRESS=1`).

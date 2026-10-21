# Concurrency and writer throughput (§11)

This document matches [README.md](../README.md), [DECISIONS.md](../DECISIONS.md) §6.3 and §9, and the engine commit path.

## What wisp guarantees

- **Section-scope optimism:** headless commits carry a **base changeset** (or section commitment). If another writer advanced the same **scope** first, the commit fails with **exit 3** (conflict JSON), not silent merge.
- **Disjoint sections:** writers editing different sections on the same ref can commit in parallel **in principle**; integration tests cover **8 concurrent headless writers** on disjoint sections (`i16_i17_concurrent_headless_writers_disjoint_sections` in `crates/wisp-engine/src/tests.rs`).
- **Exit 7:** SQLite **busy timeout** on the ledger write lock surfaces as a **retryable** error; `commit.retries` in manifest policy re-runs the attempt (see `crates/wisp-engine/src/commit.rs`).

## What wisp does not guarantee

- **Not lock-free:** the **ledger** uses SQLite with `BEGIN IMMEDIATE` on commit (`crates/wisp-store/src/ledger.rs`). Writers **serialize** at the database layer even when their logical edits do not conflict.
- **Managed checkout rewrites:** `.wisp/worktree.lock` (SQLite advisory lock) serializes **full-tree re-renders** across processes after record/commit (`DECISIONS.md` §9).
- **Not a swarm database:** many agents hammering the **same ref** with ledger-heavy commits will **queue** on SQLite; use **branches/worktrees**, **disjoint sections**, or **batch/apply** patterns when throughput matters.

## Commit ordering (hot vs ledger)

Inside the critical section, this build commits **hot tier first, then ledger** (`DECISIONS.md` §6.3). A crash between the two leaves hot ahead of ledger; the next `Repository::open` rebuilds hot from the ledger.

## Optional stress probe

Larger disjoint-writer runs are **not** default CI:

```sh
WISP_STRESS=1 WISP_STRESS_WRITERS=50 cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers
```

See [contrib/concurrency/README.md](../contrib/concurrency/README.md).

## CLI exit codes (§16)

| Code | Meaning |
|------|---------|
| 3 | Optimistic conflict |
| 7 | Retryable write timeout (lock contention) |

Full list: [README.md](../README.md) (Five-minute tour / JSON section).

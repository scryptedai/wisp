# Worktree materialization

Managed checkouts (primary bundle root and `wisp worktree` paths) are **caches** of the hot tier for a ref. The ledger is authoritative; disk is updated asynchronously unless a barrier runs.

## Publish set (delta)

After any successful commit (`record`, `headless` / `apply`, merge replay), wisp computes a **publish set** from the changeset:

- Section / frontmatter / source / asset ops → dirty concept or asset paths
- Directory prefixes → regenerate `index.md` / `log.md` for those dirs only

`checkout::publish_ref_delta` renders **only** dirty paths from hot. For headless commits it skips files with **unrecorded edits** (hash mismatch in `checkout.idx`). After `record` or merge replay, the publish set **reconciles** those paths from hot because the commit incorporated disk state.

Full-tree render remains for explicit operations: `wisp checkout`, `wisp switch`, erase recovery, and `schedule_materialize_full` after merge when the primary ref matches.

## Async drain (manifest `[materialize]`)

| Field | Default | Meaning |
|-------|---------|---------|
| `enabled` | `true` | Enqueue publish work after commits |
| `tick_ms` | `250` | Heartbeat interval for the materialize worker |

On commit, jobs are **coalesced** per `(tree_root, ref)`. A background thread wakes immediately and on each tick while backlog is non-zero. Typical lag is under ~500 ms on normal hardware (not a hard real-time guarantee).

`wisp record` **drains** the queue before returning so the working tree matches hot for the next edit. `headless` / `apply` do not (use `wisp materialize --wait` or hot APIs).

### Barriers

- `Repository::drop` / process exit — drain queue on the handle
- `wisp materialize --wait` / `Repository::flush_materialize()`
- `wisp push` / cold pack — `flush_cold_payloads` + `flush_materialize`

### Agent contract

- **Truth:** `wisp cat`, search, and engine APIs read hot.
- **Disk:** tools that only read paths should call `wisp materialize --wait` or poll `wisp status --json` → `materialize.pending_jobs` / `materialize.lag_ms`.

## Index files

| Tree | Index path |
|------|------------|
| Primary | `.wisp/checkout.idx` |
| Linked worktree | `.wisp-checkout.idx` in that directory |

See [docs/ERASE.md](ERASE.md) for managed checkout rewrite on erase, and [docs/CONCURRENCY.md](CONCURRENCY.md) for `worktree.lock`.

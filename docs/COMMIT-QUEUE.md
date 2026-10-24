# Commit queue (optional multi-agent path)

By default **`[queue] enabled` is false**: headless writes commit directly against `ledger.db` (one SQLite writer lock per process). For **many agents on one bundle**, enable the queue and run a dedicated writer.

## Enable

```toml
[queue]
enabled = true
```

Or: `wisp config set queue.enabled true` (then restart clients).

With the queue on, `wisp apply` and `headless::apply` **submit** to `.wisp/commit.sock` and **await** JSON (or **exit 11** `queue_timeout`). Set **`enabled = false`** to return to direct commits.

## Roles

| Role | What it does |
|------|----------------|
| **Queue writer** | One process holds `ledger.db` (`wisp queue --serve`). Listens on **`.wisp/commit.sock`**. |
| **Agent / CLI client** | Does not open the ledger for writes. One JSON line per `ApplyBatch`; blocks until the writer replies. |

Clients see **submit → await** (RPC-style). The writer may **coalesce** disjoint ops in a drain window.

## Start the writer

```sh
wisp queue --serve
```

One writer per bundle. Run under a supervisor in fleets.

## Client pattern (agent fleets)

1. Enable `[queue]` and start **`wisp queue --serve`**.
2. Agents use normal **`wisp apply`** / **`headless::apply`** on the bundle path.
3. **Exit 11** → retry the batch. **Exit 3** → refresh bases (unchanged).

**Socket:** request `{"batch":<ApplyBatch>,"timeout_ms":120000}`; response `{"ok":true,"data":...}` or `{"ok":false,"error":...}`. Engine: `commit_queue::submit_via_socket`, `CommitQueue::spawn` (in-process).

## Manifest (`[queue]`)

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `false` | Route `apply` through the queue |
| `timeout_ms` | `120000` | Client wait for the writer |
| `coalesce_window_us` | `500` | Writer autobatch window |
| `coalesce` | `true` | Merge disjoint single-op batches (same actor) |

## Errors

| Code | When |
|------|------|
| **11** `queue_timeout` | Client deadline (retryable) |
| **7** `write_timeout` | Writer-internal SQLite busy |
| **3** `conflict` | Precondition failed at commit |

## Battery

```sh
cargo test -p wisp-invariants battery_commit_queue -- --nocapture
WISP_COMMIT_QUEUE_BATTERY=1 cargo test -p wisp-invariants battery_commit_queue_swarm -- --ignored --nocapture
```

See [`contrib/battery-commit-queue/README.md`](../contrib/battery-commit-queue/README.md).

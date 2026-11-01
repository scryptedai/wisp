# Agent runtime (`wisp runtime --serve`)

One long-lived process per bundle owns `ledger.db`, the ref `hot.db`, and an in-memory answer cache. Agent CLIs talk to **`.wisp/runtime.sock`**. They do not open SQLite for reads or writes when `[runtime] enabled = true`.

`wisp serve` remains ColdPack replica sync. Agent fleets use **`wisp runtime --serve`** only (there is no `wisp queue` subcommand).

## Start

```sh
wisp --repo "$BUNDLE" --actor process:runtime/0.1 runtime --serve
```

```toml
[runtime]
enabled = true
timeout_ms = 120000
coalesce_window_us = 500
coalesce = true
```

Start the runtime before agents. A client with `enabled = true` and no socket **fails** (exit 2). There is no fallback that opens the ledger from an agent process.

The serve process holds **one apply writer thread** (serialized commits, optional same-actor coalesce) and serves **reads from cache** without opening SQLite in agent processes.

## Protocol

One JSON line in, one envelope out (same shape as `--json` CLI):

```json
{ "cmd": "ping", "actor": "agent/0.1", "args": {} }
```

```json
{ "ok": true, "command": "ping", "data": { "seq": 1, "head": "01J…" } }
```

Commands: `ping`, `cat`, `apply`, `pack`, `search`, `why`, `impact`, `tokens`, `feed`.

Pack args include `since_seq`, `body` (default true). When `since_seq` equals the published runtime `seq` and `body` is false, the reply is `{fresh: false, seq, head}` without text. Cache hits set `fresh: false`. When `since_seq` is older and only **append/prepend** commits landed since then, the runtime stitches cached pack/cat entries and may return a `delta` array (fragment per section) without rebuilding from SQLite.

Apply runs on a writer thread with optional coalesce (`runtime.coalesce`, `runtime.coalesce_window_us`).

`feed` over the socket returns ledger events via `query::feed`: `{since_seq, seq, runtime_seq, events}`. `seq` is the ledger head for the next `--since`; `runtime_seq` is the publish counter used by `pack`. Line streaming uses the CLI `wisp feed` when runtime is off.

`runtime.allow_human_direct` (default true): `human:*` actors may open SQLite for admin commands (`record`, `status`, `merge`, …) while the runtime is up. Tool agents must use the socket.

## Reads vs writes

- **Apply** runs single-threaded on the ledger (with optional same-actor coalesce).
- **Cat / pack / search** use the snapshot at a published `seq` and the memory cache; they do not `BEGIN IMMEDIATE` on the ledger.
- After a successful apply, the runtime bumps `seq`, updates published **heads**, invalidates or stitches cache keys, and publishes.
- **Cache-hit** `cat` / `pack` / `search` replies are served from memory **without locking the repo** (no SQLite on the hot path).
- **Cache-miss** reads use `try_lock` on the repo; if a commit holds the store, the client gets a **retryable** exit 7 (`runtime.read_try_lock`) instead of blocking for seconds.

## CLI

| Command | Role |
|---------|------|
| `wisp runtime --serve` | Owns ledger, hot, cache, and the apply writer (required when `runtime.enabled`) |
| `wisp cat`, `apply`, `pack`, … | Socket RPC when runtime is enabled |
| `wisp serve` | ColdPack replica (unchanged) |

## Acceptance battery

```sh
cargo test -p wisp-invariants --test battery_runtime -- --nocapture
WISP_RUNTIME_BATTERY=1 cargo test -p wisp-invariants --test battery_runtime -- --ignored --nocapture --test-threads=1
```

Edit scenarios in `contrib/battery-runtime/cases.toml` only.

# Commit queue battery

Compares **direct** multi-process commits (`[queue] enabled = false`) with the **queued** path (single writer, optional autobatch).

```sh
cargo test -p wisp-invariants battery_commit_queue -- --nocapture
```

**100-agent swarm** (disjoint sections S0–S99, 500 commits):

```sh
WISP_COMMIT_QUEUE_BATTERY=1 cargo test -p wisp-invariants battery_commit_queue_swarm -- --ignored --nocapture
```

Edit scenarios only in `cases.toml`.

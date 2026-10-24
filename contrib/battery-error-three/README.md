# Battery error-three (exit 3 + merge)

Timing and outcome probes for **append**, **conflict detection** (exit 3), and **resolution** (successful retry or `wisp merge`), in milliseconds.

## Section 1 (this battery)

Synthetic bundle `battery-error-three` / `notes/swarm-metrics.md`:

| Phase | Pattern | Intent |
|-------|---------|--------|
| **S1-single-five** | 5 sequential updates, one actor, **Frontier** | Baseline append latency (no conflict) |
| **S1-double-five** | 5 rounds: two actors, same **Frontier** section; B uses stale base after A | Exit **3** detection, then base refresh (resolution) |
| **S1-triple-five** | 5 rounds: three actors, **Lane-Alpha / Beta / Gamma** on separate branches, merge to `main` | Independent information → **merge** (no ours/theirs) |

Behavior tree and injected paragraph text: **`cases.toml`** (single source of truth).

## Run

```sh
cargo test -p wisp-invariants battery_error_three_section_one -- --nocapture
```

Stdout prints a table of `append_ms`, `conflict_detect_ms`, and `resolution_ms` per step.

## Edit contract

Change probes only in `contrib/battery-error-three/cases.toml`; implement runner logic in `crates/wisp-invariants/tests/battery_error_three.rs`.

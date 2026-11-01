# Runtime acceptance battery

Scenarios in `cases.toml` (idle cat, warm pack, append delta, read-during-write, apply envelope, no-socket misconfig).

```sh
cargo test -p wisp-invariants --test battery_runtime -- --nocapture
```

Heavy multi-client scenarios:

```sh
WISP_RUNTIME_BATTERY=1 cargo test -p wisp-invariants --test battery_runtime -- --ignored --nocapture --test-threads=1
```

Edit scenarios in `contrib/battery-runtime/cases.toml` only.

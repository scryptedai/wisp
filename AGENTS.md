# AGENTS.md

Guidance for AI coding agents (Cursor, CodeRabbit, Claude Code, etc.) working on **wisp Community Release** — GPLv2, Copyright Scrypted.

## What this is

**wisp** is a local-first differential OKF (Open Knowledge Format) store: section-level changesets, encrypted cold storage, hot SQLite tiers, agent-friendly CLI (`wisp --json`). Release **0.1.0** retrieval: **`docs/SEARCH.md`**. Optional embedding backends: **`docs/EXTENSIONS.md`**. OKF/store deltas: **`DECISIONS.md`** — read before changing behavior.

Do not confuse:

| Term | Meaning |
|------|---------|
| **git worktree** | Extra checkout of *this source repo* for parallel branches (`git worktree add …`) |
| **`wisp worktree`** | OKF bundle linked checkout under `.wisp/worktrees/` (product feature) |

## Repository layout

| Path | Role |
|------|------|
| `crates/wisp-cli` | `wisp` binary, clap dispatch |
| `crates/wisp-engine` | Record, checkout, merge, erase, query |
| `crates/wisp-store` | Ledger + hot SQLite |
| `crates/wisp-crypto` | Payload encryption, keystore, Ed25519 |
| `crates/wisp-corpus` | OKF bundles, scenarios, `catalog.toml` |
| `crates/wisp-invariants` | Integration tests (CI gate) |
| `crates/wisp-sync`, `wisp-git`, `wisp-feed`, `wisp-sub-*`, `wisp-index` | M5–M8 modules |
| `DECISIONS.md` | Spec deviations (required reading for behavior changes) |
| `docs/SEARCH.md`, `docs/EXTENSIONS.md`, `docs/MERGE.md` | Built-in search, extension API, merge rules |

## Build and test (required before PR)

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo test -p wisp-invariants
cargo build --release
```

Rust **1.88+** (see workspace `rust-version`). If `libsqlite3-sys` bindgen fails in a sandbox, use  
`CARGO_TARGET_DIR=./target cargo test --workspace`.

## TDD contract

1. Add or extend fixtures in **`wisp-corpus`** (`catalog.toml`, bundles, scenarios) when behavior is user-visible.
2. Add or extend tests in **`wisp-invariants`** or `wisp-engine` tests.
3. Implement until tests pass.
4. Record spec deviations in **`DECISIONS.md`** (§18 style).

SPY desk narrative: `wisp-invariants/tests/spy_options_*.rs`, bundle `spy-options`.

Search IR battery: edit **`contrib/search-battery/cases.toml` only** (Rust `search_battery.rs` and `run_battery.py` load it). See `contrib/search-battery/README.md`.

Merge rule battery: edit **`contrib/merge-rules/cases.toml` only**; tests in `wisp-invariants/tests/merge_rules.rs`. See `contrib/merge-rules/README.md` and `docs/MERGE.md`.

## Coding norms

- Match existing style in the file you edit; minimal diffs.
- GPLv2 Community Release: preserve `LICENSE` / copyright headers on new files where the project uses them.
- Do not add proprietary or alternate licenses in this repo.
- Prefer `wisp-engine` APIs over duplicating ledger/hot logic in CLI.
- Section-level concurrency: disjoint sections should not conflict; preserve optimistic commit preconditions.

## CLI and JSON

- Commands support `--json`; integration tests often drive `wisp-engine` directly.
- Actor: `human:*`, `process:*`, or `<tool>/<version>` — see `WispError` / manifest policies.

## What not to do

- Do not edit generated OKF `index.md` / `log.md` in corpus bundles unless the test expects it.
- Do not remove `wisp_id` or section anchors in fixtures without updating matchers.
- Do not commit secrets (`.wisp/keys`, `WISP_MASTER_KEY` in files).
- Do not change `DECISIONS.md` or public CLI contracts without tests.

## Automation

- **CI**: `.github/workflows/ci.yml` — fmt, clippy, test all targets, release build on three OSes.
- **CodeRabbit**: `.coderabbit.yaml` — ignore `target/`, lockfile noise; focus on `crates/`.

## Human contribution flow

See **`CONTRIBUTING.md`** — GitHub Flow (`main` + topic branches + PRs; no `develop` branch).

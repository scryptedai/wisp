# Search IR battery

Five probe categories (lexical, typo/fuzzy, semantic gap, polysemy, acronym) over the `search-battery` OKF corpus in `wisp-corpus`.

## Single source of truth

**Edit probes only in [`cases.toml`](cases.toml).** Both runners load this file and apply the same assertion rules:

| Runner | Role |
|--------|------|
| `cargo test -p wisp-invariants search_battery` | CI gate (in-process `wisp-engine`) |
| `contrib/search-battery/run_battery.py` | CLI smoke against a real `wisp` binary |

When you add or change a probe, update `cases.toml` once, then run both runners.

### Contract (default `semantic` build)

| Probe | `auto` | `fuzzy` / `keyword` / `ri` | `auto --local-only` |
|-------|--------|----------------------------|---------------------|
| T1.0 lexical | rank 1 | (mode-specific; see `should_assert`) | same as `auto` without neural |
| T2.x | trigram/fuzzy where applicable | keyword skips typo T2.1 | — |
| **T3.1** paraphrase → OOM | **rank 1** (`mode_used: semantic`) | miss | **miss** |
| **T3.2** GDPR → shred | miss | miss | miss |

Documented in **`docs/references/Differential OKF Store — v1 Technical Design.md`** §14.7 and **`README.md`** (search tour).

Fields in `cases.toml`:

- `expect`: `top1` | `topK` | `miss@K` | `report`
- `keyword_in_top_k`: whether `keyword` mode asserts `topK`
- `auto_semantic_top1`: when neural is enabled, `auto` / `semantic` require rank 1 instead of `miss@K` (T3.1)

Printed expect column shows `top1@neural` when that override applies.

## Rust (CI)

```sh
cargo test -p wisp-invariants search_battery -- --nocapture
```

Air-gapped / no neural (matches `--airgap` on the Python runner):

```sh
cargo test -p wisp-invariants --no-default-features search_battery -- --nocapture
```

Set `FASTEMBED_CACHE_DIR` in CI to cache the ~128 MiB embedding model between runs.

## CLI (Python runner)

```sh
cp -R crates/wisp-corpus/data/bundles/search-battery /tmp/wisp-search-bat
cd /tmp/wisp-search-bat && wisp init && wisp record -m corpus
export WISP_BUNDLE_ROOT=$PWD
export PATH=/path/to/wisp/target/release:$PATH

python3 contrib/search-battery/run_battery.py all          # all modes; exit 1 on failure
python3 contrib/search-battery/run_battery.py auto
python3 contrib/search-battery/run_battery.py all --airgap # no neural expectations
python3 contrib/search-battery/run_battery.py auto --local-only  # + T3.x miss checks
```

Requires **Python 3.11+** (`tomllib`).

# Search IR battery

Five probe categories (lexical, typo/fuzzy, semantic gap, polysemy, acronym) over the `search-battery` OKF corpus in `wisp-corpus`.

T3.x must **miss@5** on keyword, vector, and hybrid (no RRF ghost hits). T2.1 typos with no shared tokens are informational only until a character-level or neural codec ships.

**Rust (CI):**

```sh
cargo test -p wisp-invariants search_battery -- --nocapture
```

**CLI (after init + record in a copy of the bundle):**

```sh
cp -R crates/wisp-corpus/data/bundles/search-battery /tmp/wisp-search-bat
cd /tmp/wisp-search-bat && wisp init && wisp record -m corpus
export WISP_BUNDLE_ROOT=$PWD
python3 contrib/search-battery/run_battery.py hybrid
```

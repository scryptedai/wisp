# Search (built-in)

wisp **0.1.0** answers section queries from the **hot tier** only. All built-in indexes live in `cache/refs/<ref-id>/hot.db`. There is one search product surface: `wisp search`.

## Modes

| Mode | Behavior |
|------|----------|
| `auto` | Default pipeline below |
| `exact` | `#anchor` or `"quoted phrase"` substring match |
| `keyword` | BM25 on `state_search_fts` |
| `fuzzy` | BM25 + character trigram, RRF merge |
| `ri` | Random indexing (incremental TF–IDF, 256-d sparse vectors in hot.db) |
| `semantic` | Dense embeddings in `section_dense_embeddings` (requires `semantic` build feature) |

Unknown modes are rejected. There are no alternate names.

## `auto` pipeline

1. **Exact** — stop if hits.
2. **Lexical + fuzzy** — BM25 + trigram FTS candidates, char-trigram coverage, RRF (k = 60), precision gate on multi-token queries.
3. **RI** — cosine over `ri_*` tables when step 2 is empty.
4. **Semantic** — BGE-small-en-v1.5 (`fastembed`) when step 3 is empty, binary built with `semantic`, and not local-only.

`--local-only` or `WISP_LOCAL_ONLY=1` skips step 4 only. RI still runs.

`--threshold` adjusts fuzzy and RI/neural score floors where applicable.

## Field search (`--field`)

`wisp search --field heading|body|title|path` matches one index surface:

| Field | Source |
|-------|--------|
| `heading` | `state_sections.heading` (`level > 0` only) |
| `body` | Section body, including the preamble (`level = 0`) |
| `title` | Frontmatter entry with key `title` |
| `path` | Concept path (`state_concepts.path`) |

With `--field`, modes are `auto` (exact then keyword), `exact` (substring), or `keyword` (FTS on `heading`/`body` columns only; `title`/`path` use substring). Each hit names `field`, a short `snippet`, and the section `version` at query time.

## JSON (`wisp search --json`)

Top-level: `mode`, `requested_mode`, `query`, `local_only`, `results` (array). With `--field`, `field` repeats the flag value.

Each result object always includes `concept`, `path`, `section`, `heading`, `score`, `tokens`. With `--field`, hits also include `field`, `snippet`, and `version` (hot section change id at query time — use as `base_version` on the next write).

## Neural tier

- **Model:** `bge-small-en-v1.5` (tag stored in `section_dense_embeddings.model_tag`).
- **Cache:** `FASTEMBED_CACHE_DIR`, else `.wisp/cache/models/`.
- **Refresh:** embedding rows are dropped on section upsert and rebuilt on the next semantic query.
- **Air-gapped binary:** `cargo install wisp-cli --no-default-features` — no step 4; `semantic` mode returns a usage error.

## Erase

Erasing a section removes its rows from FTS, trigram FTS, RI tables, and `section_dense_embeddings` in hot.db.

If the manifest defines **extension** `[vector_index.backends]` entries (see `docs/EXTENSIONS.md`), erase also purges those external indexes per codec. A stock `wisp init` repo has none.

## Evaluation

Canonical probes: `contrib/search-battery/cases.toml` (Rust `wisp-invariants` + `contrib/search-battery/run_battery.py`).

## Extensions

Built-in search **does not** read `manifest.toml` `default_embedding` or manifest embedding codecs. Products that store vectors elsewhere implement `wisp_index::EmbeddingCodec` + `VectorIndexBackend` as documented in **`docs/EXTENSIONS.md`**.

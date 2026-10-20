# Extension API

Community **wisp** ships one retrieval stack (see **`docs/SEARCH.md`**). This document defines **optional** hooks for products that store embeddings outside hot.db or use remote models.

Built-in `wisp search` never calls these traits.

## Embedding codecs

Trait: `wisp_index::EmbeddingCodec`

| Method | Role |
|--------|------|
| `id()` | Manifest codec string (`emb:<provider>:<model>@<rev>/<dims>`) |
| `dimension()` | Vector length |
| `embed(text)` | Dense vector |

Register codecs in `manifest.toml` under `[codecs.embedding]` and wire a matching `[vector_index.backends."<id>"]` block. Implement the trait in your crate; load it from a `command` subscriber or your own binary.

## Vector index backends

Trait: `wisp_index::VectorIndexBackend`

| Method | Role |
|--------|------|
| `upsert` / `replace_all` | Sync from hot tier or an external ETL |
| `delete_sections` | Purge on erase |
| `search` | Similarity query (your product’s search UX) |

Example manifest:

```toml
[codecs]
embedding = ["emb:openai:text-embedding-3-small@1/1536"]
default_embedding = "emb:openai:text-embedding-3-small@1/1536"

[vector_index.backends."emb:openai:text-embedding-3-small@1/1536"]
driver = "postgres"
url_env = "WISP_VECTOR_DSN"
```

| `driver` | Reference implementation |
|----------|-------------------------|
| `sqlite` | `wisp-sub-embed::SqliteVectorBackend` (`.wisp/cache/vectors/<codec>.db`) |
| `postgres` / `pgvector` | `crates/wisp-vector-pg` — `wisp-cli --features postgres-vector` |
| `command` | Your process implementing JSON-lines `VectorIndexBackend` |

On erase, the engine calls `delete_sections` on **every** configured `[vector_index.backends]` entry.

## Sync transports

| URL | Behaviour |
|-----|-----------|
| `file:///path`, plain path | Local replica (`wisp-sync::local`) |
| `https://host/repo` | Dumb HTTP: `GET/PUT {base}/wisp/v1/pack.zst` |
| `ssh://user@host/path` | `rsync` + local `wisp pull`/`push` |

## Git interop

| Command | Behaviour |
|---------|-----------|
| `export --archive out.tar.zst` | Render ref → tar.zst |
| `import --git <repo>` | Snapshot via `git archive`, or `--history` for per-commit import |
| `wisp serve --http ADDR` | Dumb `GET`/`PUT` `/wisp/v1/pack.zst` |
| `export --git <dir>` | Single-commit git repo |

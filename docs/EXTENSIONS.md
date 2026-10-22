# Extension API

Community **wisp** ships one retrieval stack (see **`docs/SEARCH.md`**). This document defines **optional** hooks for products that store embeddings outside hot.db or use remote models.

Built-in `wisp search` never calls these traits.

## Community 0.1.x non-goals

| Topic | Status |
|-------|--------|
| **MCP / JSON-RPC server** | Not shipped. Agent integration is the **`wisp` CLI** (`--json`), optional skills under `contrib/`, and subprocess orchestration by your harness. |
| **`serve --stdio`** | **Not MCP.** Push: `Hello.transport=cold_pack` + PushRef → Ack → client **ColdPack** → Done. Pull: `cold_pack_pull` + PullRef → Ack → server **ColdPack** → Done. Default `local_path` → third-line bundle path on the same machine. CLI: `wisp push` / `wisp pull` with `stdio://…` remotes, or `wisp_sync::client_push_cold_pack` / `client_pull_cold_pack`. |
| **Model-exact tokenizers** | Default build uses **`tok:approx:chars4`**; pluggable token codecs are below. Tiktoken/Hugging Face IDs may be registered in manifest for forward compatibility; community binary resolves built-in approx codecs only unless you add a subscriber. |

## Token codecs

Trait: `wisp_sub_tokens::TokenCodec`

| Method | Role |
|--------|------|
| `id()` | Manifest string (`tok:approx:chars4`, …) |
| `count(text)` | Token estimate for `wisp pack --budget` and `wisp tokens` |

Register in `manifest.toml`:

```toml
[codecs]
tokenizer = ["tok:approx:chars4", "tok:approx:bytes4"]
default_tokenizer = "tok:approx:chars4"
```

CLI: `wisp codec add|default|list|remove`. Built-in community codecs:

| ID | Rule |
|----|------|
| `tok:approx:chars4` | `ceil(char_count / 4)` (default) |
| `tok:approx:bytes4` | `ceil(utf8_byte_len / 4)` |

`wisp pack --json` reports `codec`, `budget_safe` (90% of `--budget`), and `codec_note` when using approx codecs.

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

See **[docs/GIT-INTEROP.md](GIT-INTEROP.md)** for anchor safety and export/import limits.

| Command | Behaviour |
|---------|-----------|
| `export --archive out.tar.zst` | Render ref → tar.zst |
| `import --git <repo>` | Snapshot via `git archive`, or `--history` for per-commit import |
| `wisp serve --http ADDR` | Dumb `GET`/`PUT` `/wisp/v1/pack.zst` |
| `export --git <dir>` | Single-commit git repo |

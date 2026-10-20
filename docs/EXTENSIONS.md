# Extension API (embeddings, vector index, sync)

The default `wisp` binary ships a **reference** stack: SQLite ledger/hot, `emb:hash:64`, and a SQLite vector table under `.wisp/cache/vectors/`. Products can replace or augment pieces without forking core.

## Repository layout (`wisp init`)

Like `git init`, `wisp init` creates **`.wisp/`** in the bundle root and treats the surrounding directory as the OKF working tree. Commands discover the nearest `.wisp/` upward from the current directory.

## Embedding codecs

Trait: `wisp_index::EmbeddingCodec`

| Method | Role |
|--------|------|
| `id()` | Manifest codec string (`emb:…`) |
| `dimension()` | Vector length |
| `embed(text)` | Dense vector |

Built-in: `emb:hash:64` in `wisp-sub-embed` — bag-of-token cosine similarity with a minimum score floor (`HASH_MIN_COSINE`); typo/paraphrase limits and the `search-battery` corpus are documented in `DECISIONS.md` §14.

Register additional codecs in `manifest.toml` under `[codecs]` and implement the trait in your crate. Wire search via `wisp engine` or a `command` subscriber that updates an external index.

## Vector index backends

Trait: `wisp_index::VectorIndexBackend`

| Method | Role |
|--------|------|
| `upsert` / `replace_all` | Sync from hot tier or an external ETL |
| `delete_sections` | Purge on erase (core calls this for the default backend) |
| `search` | Similarity query |

Configure per codec in `manifest.toml`:

```toml
[vector_index.backends."emb:hash:64"]
driver = "sqlite"
# path = "cache/vectors/custom.db"   # optional; default derived from codec id

[vector_index.backends."emb:openai:3-small"]
driver = "postgres"
url_env = "WISP_VECTOR_DSN"
```

| `driver` | Status in reference binary |
|----------|----------------------------|
| `sqlite` | Implemented (`wisp-sub-embed`) |
| `postgres` / `pgvector` | `crates/wisp-vector-pg` — build `wisp` with `--features postgres-vector` |
| `command` | Extension point — JSON-lines helper on stdin/stdout |

## Erase / vector purge

When scopes are shredded, the engine collects section ids in those scopes and calls `delete_sections` on the configured default embedding index. External Postgres indexes should subscribe to the same feed events or mirror this hook.

## Sync transports

| URL | Behaviour |
|-----|-----------|
| `file:///path`, plain path | Trusted local replica copy (`wisp-sync::local`) |
| `https://host/repo` | Dumb HTTP: `GET/PUT {base}/wisp/v1/pack.zst` (cold-tier tar.zst) |
| `ssh://user@host/path` | `rsync -az -e ssh` of bundle dir + `wisp pull`/`push` file transport |

HTTP expects your product to serve or accept the pack at that path (similar to git dumb HTTP). SSH expects `rsync`, `ssh`, and `wisp` on the remote for the rsync-based path.

## Git interop

| Command | Behaviour |
|---------|-----------|
| `export --archive out.tar.zst` | Render ref → tar.zst (like `git archive`) |
| `import --git <repo>` | Snapshot via `git archive`, or `--history` for one changeset per commit + `.wisp/meta/git-map.json` |
| `wisp serve --http ADDR` | Dumb `GET`/`PUT` `/wisp/v1/pack.zst` for HTTPS remotes |
| `export --git <dir>` | Single-commit git repo |

Per-commit git history import remains a product extension; snapshot import is built in.

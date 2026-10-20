# wisp-vector-pg

Postgres backend for `wisp_index::VectorIndexBackend` (extension API — see `docs/EXTENSIONS.md`). Not used by built-in `wisp search`.

```sh
cargo build -p wisp-cli --features postgres-vector
```

```toml
[vector_index.backends."emb:openai:text-embedding-3-small@1/1536"]
driver = "postgres"
url_env = "WISP_VECTOR_DSN"
```

Apply `src/schema.rs` (`PGVECTOR_DDL`) in your database.

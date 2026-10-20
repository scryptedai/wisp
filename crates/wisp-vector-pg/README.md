# wisp-vector-pg

Sample **Postgres** backend for [`wisp_index::VectorIndexBackend`](../wisp-index).

## Build

```sh
cargo build -p wisp-cli --features postgres-vector
```

## Configure

```toml
[vector_index.backends."emb:hash:64"]
driver = "postgres"
url_env = "WISP_VECTOR_DSN"
```

```sh
export WISP_VECTOR_DSN='postgresql://user@localhost/wisp'
```

## pgvector

For production cosine indexes, apply `src/schema.rs` → `PGVECTOR_DDL` and switch the Rust impl to the `vector` column type (fork this crate or add a `pgvector` feature).

//! DDL for operators shipping pgvector in production.

/// Recommended pgvector schema (run once per database).
pub const PGVECTOR_DDL: &str = r"
CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE IF NOT EXISTS wisp_vectors (
  codec   TEXT NOT NULL,
  section BYTEA NOT NULL,
  concept BYTEA NOT NULL,
  version BYTEA NOT NULL,
  path    TEXT NOT NULL,
  heading TEXT NOT NULL,
  body    TEXT NOT NULL,
  embedding vector(64) NOT NULL,
  PRIMARY KEY (codec, section)
);
CREATE INDEX IF NOT EXISTS wisp_vectors_embedding_idx
  ON wisp_vectors USING ivfflat (embedding vector_cosine_ops) WITH (lists = 100);
";

/// Default reference build uses `REAL[]` (no extension required).
pub const REAL_ARRAY_DDL: &str = r"
CREATE TABLE IF NOT EXISTS wisp_vectors (
  codec   TEXT NOT NULL,
  section BYTEA NOT NULL,
  concept BYTEA NOT NULL,
  version BYTEA NOT NULL,
  path    TEXT NOT NULL,
  heading TEXT NOT NULL,
  body    TEXT NOT NULL,
  embedding BYTEA NOT NULL,
  PRIMARY KEY (codec, section)
);
";

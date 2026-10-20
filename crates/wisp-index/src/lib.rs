//! Extension API for embeddings and vector search (see `docs/EXTENSIONS.md`).
//!
//! Reference implementations live in `wisp-sub-embed` (SQLite). Products can supply
//! Postgres/pgvector or remote embedding services by implementing these traits and
//! registering backends in `manifest.toml`.

use wisp_model::{ConceptId, Result, SectionId};

/// One indexed section row (content + embedding).
#[derive(Clone, Debug)]
pub struct SectionVectorRow {
    pub concept: ConceptId,
    pub section: SectionId,
    pub version: wisp_model::ChangeId,
    pub path: String,
    pub heading: String,
    pub body: String,
    pub vector: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct VectorSearchHit {
    pub concept: ConceptId,
    pub section: SectionId,
    pub path: String,
    pub heading: String,
    pub body: String,
    pub score: f32,
}

/// Text → dense vector for a named codec (`emb:openai:…`, …). Not used by built-in `wisp search`.
pub trait EmbeddingCodec: Send + Sync {
    fn id(&self) -> &str;
    fn dimension(&self) -> usize;
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// Vector storage and similarity search (SQLite, Postgres/pgvector, …).
pub trait VectorIndexBackend: Send {
    fn codec_id(&self) -> &str;
    /// Replace or insert rows; idempotent on `(section, version)`.
    fn upsert(&mut self, rows: &[SectionVectorRow]) -> Result<()>;
    fn delete_sections(&mut self, sections: &[SectionId]) -> Result<()>;
    /// Drop all rows and rebuild from `rows` (reference sync strategy).
    fn replace_all(&mut self, rows: &[SectionVectorRow]) -> Result<()>;
    fn search(&self, query: &[f32], k: usize) -> Result<Vec<VectorSearchHit>>;
    fn checkpoint(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Built-in driver names in `manifest.toml` `[vector_index.backends]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorDriver {
    Sqlite,
    /// Declared for external crates (`wisp-vector-pg`); not implemented in the default binary.
    Postgres,
    ExternalCommand,
}

impl VectorDriver {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sqlite" => Some(VectorDriver::Sqlite),
            "postgres" | "postgresql" | "pgvector" => Some(VectorDriver::Postgres),
            "command" => Some(VectorDriver::ExternalCommand),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct VectorBackendConfig {
    pub driver: String,
    /// For `sqlite`, relative path under `.wisp/cache/vectors/` (default: derived from codec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For `postgres`, env var holding DSN (product extension).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_env: Option<String>,
    /// For `command`, argv to a helper that speaks JSON lines on stdin/stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

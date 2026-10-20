//! `wisp-sub-embed`: the embedding subscriber and vector index (§14.4). Not implemented in this build (M5).
//!
//! Vectors are operational search accelerators that belong to the hot tier, never history. They
//! are stored in `.wisp/cache/vectors/<codec>.db` with `sqlite-vec`, keyed by section version, and
//! deleted synchronously in erase Phase 1.

pub const CODEC_PREFIX: &str = "emb:";

//! `wisp-sub-embed`: deterministic hash embeddings and a section vector index (§14.4).
//!
//! Vectors are search accelerators under `.wisp/cache/vectors/<codec>.db`, keyed by section id
//! and content version. This build ships `emb:hash:64` (no external model).

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;
use wisp_model::{ConceptId, Result, SectionId, WispError};
use wisp_store::Hot;

pub const CODEC_PREFIX: &str = "emb:";
pub const HASH_CODEC: &str = "emb:hash:64";
pub const DIM: usize = 64;

pub fn is_supported(codec: &str) -> bool {
    codec == HASH_CODEC
}

/// Deterministic bag-of-words hash embedding, L2-normalized.
pub fn embed_text(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text.split_whitespace() {
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        let digest = h.finalize();
        let bucket = (digest[0] as usize) % dim;
        let sign = if digest[1] & 1 == 0 { 1.0 } else { -1.0 };
        v[bucket] += sign;
    }
    let norm = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vec(blob: &[u8], dim: usize) -> Option<Vec<f32>> {
    if blob.len() != dim * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        let b = blob[i * 4..(i + 1) * 4].try_into().ok()?;
        out.push(f32::from_le_bytes(b));
    }
    Some(out)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub struct VectorHit {
    pub concept: ConceptId,
    pub section: SectionId,
    pub path: String,
    pub heading: String,
    pub body: String,
    pub score: f32,
}

pub struct VectorIndex {
    conn: Connection,
    dim: usize,
}

impl VectorIndex {
    pub fn open(path: &Path, codec: &str) -> Result<Self> {
        if !is_supported(codec) {
            return Err(WispError::usage(format!("unsupported embedding codec {codec}")));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WispError::internal(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| WispError::internal(e.to_string()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                section BLOB PRIMARY KEY,
                concept BLOB NOT NULL,
                version BLOB NOT NULL,
                path TEXT NOT NULL,
                heading TEXT NOT NULL,
                body TEXT NOT NULL,
                vec BLOB NOT NULL
            );",
        )
        .map_err(|e| WispError::internal(e.to_string()))?;
        Ok(VectorIndex { conn, dim: DIM })
    }

    /// Rebuild the index from the current hot tier (level > 0 sections only).
    pub fn sync_from_hot(&mut self, hot: &Connection) -> Result<()> {
        self.conn
            .execute("DELETE FROM vectors", [])
            .map_err(|e| WispError::internal(e.to_string()))?;
        let tx = self
            .conn
            .transaction()
            .map_err(|e| WispError::internal(e.to_string()))?;
        for c in Hot::concepts(hot)? {
            for s in Hot::sections_of(hot, c.concept)? {
                if s.level == 0 {
                    continue;
                }
                let text = format!("{}\n{}", s.heading, s.body);
                let vec = embed_text(&text, self.dim);
                tx.execute(
                    "INSERT INTO vectors (section, concept, version, path, heading, body, vec)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        s.section.bytes().to_vec(),
                        c.concept.bytes().to_vec(),
                        s.version.bytes().to_vec(),
                        c.path,
                        s.heading,
                        s.body,
                        vec_to_blob(&vec),
                    ],
                )
                .map_err(|e| WispError::internal(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| WispError::internal(e.to_string()))?;
        Ok(())
    }

    pub fn search(&self, query_vec: &[f32], k: usize) -> Result<Vec<VectorHit>> {
        let mut st = self
            .conn
            .prepare("SELECT concept, section, path, heading, body, vec FROM vectors")
            .map_err(|e| WispError::internal(e.to_string()))?;
        let mut hits = Vec::new();
        let rows = st
            .query_map([], |r| {
                Ok((
                    ConceptId::from_bytes(blob16(r.get(0)?)),
                    SectionId::from_bytes(blob16(r.get(1)?)),
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Vec<u8>>(5)?,
                ))
            })
            .map_err(|e| WispError::internal(e.to_string()))?;
        for row in rows.filter_map(|r| r.ok()) {
            let (concept, section, path, heading, body, blob) = row;
            let Some(vec) = blob_to_vec(&blob, self.dim) else {
                continue;
            };
            let score = cosine(query_vec, &vec);
            hits.push(VectorHit {
                concept,
                section,
                path,
                heading,
                body,
                score,
            });
        }
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(k);
        Ok(hits)
    }
}

fn blob16(v: Vec<u8>) -> [u8; 16] {
    let mut b = [0u8; 16];
    if v.len() == 16 {
        b.copy_from_slice(&v);
    }
    b
}

pub fn index_path(wisp_dir: &Path, codec: &str) -> std::path::PathBuf {
    let safe = codec.replace(':', "_");
    wisp_dir.join("cache").join("vectors").join(format!("{safe}.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_is_deterministic() {
        let a = embed_text("append only rows orders", DIM);
        let b = embed_text("append only rows orders", DIM);
        assert_eq!(a, b);
        let c = embed_text("unrelated quantum physics", DIM);
        assert!(cosine(&a, &c) < 0.5);
    }
}

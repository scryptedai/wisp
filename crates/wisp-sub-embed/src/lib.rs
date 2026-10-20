//! `wisp-sub-embed`: reference embedding codec and SQLite vector backend (§14.4).

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use wisp_index::{EmbeddingCodec, SectionVectorRow, VectorIndexBackend, VectorSearchHit};
use wisp_model::{ConceptId, Result, SectionId, WispError};
use wisp_store::Hot;

pub const CODEC_PREFIX: &str = "emb:";
pub const HASH_CODEC: &str = "emb:hash:64";
pub const DIM: usize = 64;

pub fn is_supported(codec: &str) -> bool {
    codec == HASH_CODEC
}

pub fn codec_for(id: &str) -> Result<Box<dyn EmbeddingCodec>> {
    if id == HASH_CODEC {
        Ok(Box::new(HashEmbeddingCodec))
    } else {
        Err(WispError::usage(format!(
            "embedding codec {id} is not built in; register an external codec in your product"
        )))
    }
}

pub struct HashEmbeddingCodec;

impl EmbeddingCodec for HashEmbeddingCodec {
    fn id(&self) -> &str {
        HASH_CODEC
    }
    fn dimension(&self) -> usize {
        DIM
    }
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(embed_text(text, DIM))
    }
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

/// SQLite vector index (default backend).
pub struct SqliteVectorBackend {
    codec: String,
    conn: Connection,
    dim: usize,
}

impl SqliteVectorBackend {
    pub fn open(wisp_dir: &Path, codec: &str, path_override: Option<&Path>) -> Result<Self> {
        if !is_supported(codec) {
            return Err(WispError::usage(format!("unsupported embedding codec {codec}")));
        }
        let path = path_override
            .map(PathBuf::from)
            .unwrap_or_else(|| index_path(wisp_dir, codec));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WispError::internal(e.to_string()))?;
        }
        let conn = Connection::open(&path).map_err(|e| WispError::internal(e.to_string()))?;
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
        Ok(SqliteVectorBackend {
            codec: codec.to_string(),
            conn,
            dim: DIM,
        })
    }
}

impl VectorIndexBackend for SqliteVectorBackend {
    fn codec_id(&self) -> &str {
        &self.codec
    }

    fn upsert(&mut self, rows: &[SectionVectorRow]) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| WispError::internal(e.to_string()))?;
        for r in rows {
            tx.execute(
                "INSERT OR REPLACE INTO vectors (section, concept, version, path, heading, body, vec)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    r.section.bytes().to_vec(),
                    r.concept.bytes().to_vec(),
                    r.version.bytes().to_vec(),
                    r.path,
                    r.heading,
                    r.body,
                    vec_to_blob(&r.vector),
                ],
            )
            .map_err(|e| WispError::internal(e.to_string()))?;
        }
        tx.commit().map_err(|e| WispError::internal(e.to_string()))?;
        Ok(())
    }

    fn delete_sections(&mut self, sections: &[SectionId]) -> Result<()> {
        for s in sections {
            self.conn
                .execute(
                    "DELETE FROM vectors WHERE section = ?1",
                    params![s.bytes().to_vec()],
                )
                .map_err(|e| WispError::internal(e.to_string()))?;
        }
        Ok(())
    }

    fn replace_all(&mut self, rows: &[SectionVectorRow]) -> Result<()> {
        self.conn
            .execute("DELETE FROM vectors", [])
            .map_err(|e| WispError::internal(e.to_string()))?;
        self.upsert(rows)
    }

    fn search(&self, query: &[f32], k: usize) -> Result<Vec<VectorSearchHit>> {
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
            let score = cosine(query, &vec);
            hits.push(VectorSearchHit {
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

/// Legacy handle (wraps SQLite backend).
pub struct VectorIndex {
    inner: SqliteVectorBackend,
}

impl VectorIndex {
    pub fn open(path: &Path, codec: &str) -> Result<Self> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let wisp_dir = parent.parent().and_then(|p| p.parent()).unwrap_or(parent);
        let inner = SqliteVectorBackend::open(wisp_dir, codec, Some(path))?;
        Ok(VectorIndex { inner })
    }

    pub fn sync_from_hot(&mut self, hot: &Connection) -> Result<()> {
        let codec = codec_for(self.inner.codec_id())?;
        let rows = rows_from_hot(hot, codec.as_ref())?;
        self.inner.replace_all(&rows)
    }

    pub fn search(&self, query_vec: &[f32], k: usize) -> Result<Vec<VectorHit>> {
        Ok(self
            .inner
            .search(query_vec, k)?
            .into_iter()
            .map(|h| VectorHit {
                concept: h.concept,
                section: h.section,
                path: h.path,
                heading: h.heading,
                body: h.body,
                score: h.score,
            })
            .collect())
    }
}

pub fn rows_from_hot(hot: &Connection, codec: &dyn EmbeddingCodec) -> Result<Vec<SectionVectorRow>> {
    let mut out = Vec::new();
    for c in Hot::concepts(hot)? {
        for s in Hot::sections_of(hot, c.concept)? {
            if s.level == 0 {
                continue;
            }
            let text = format!("{}\n{}", s.heading, s.body);
            let vector = codec.embed(&text)?;
            out.push(SectionVectorRow {
                concept: c.concept,
                section: s.section,
                version: s.version,
                path: c.path.clone(),
                heading: s.heading.clone(),
                body: s.body.clone(),
                vector,
            });
        }
    }
    Ok(out)
}

pub fn purge_sections(wisp_dir: &Path, codec: &str, sections: &[SectionId]) -> Result<()> {
    if sections.is_empty() || !is_supported(codec) {
        return Ok(());
    }
    let path = index_path(wisp_dir, codec);
    if !path.is_file() {
        return Ok(());
    }
    let mut backend = SqliteVectorBackend::open(wisp_dir, codec, None)?;
    backend.delete_sections(sections)?;
    backend.checkpoint()?;
    Ok(())
}

fn blob16(v: Vec<u8>) -> [u8; 16] {
    let mut b = [0u8; 16];
    if v.len() == 16 {
        b.copy_from_slice(&v);
    }
    b
}

pub fn index_path(wisp_dir: &Path, codec: &str) -> PathBuf {
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

//! Tier 4: optional CPU dense embeddings (`fastembed`).

use rusqlite::Connection;
use std::path::Path;
use wisp_model::{Result, WispError};
use wisp_store::SearchHit;

#[cfg(feature = "semantic")]
pub const MODEL_TAG: &str = "bge-small-en-v1.5";

#[cfg(feature = "semantic")]
pub fn search_semantic(
    wisp_dir: &Path,
    conn: &Connection,
    query: &str,
    k: usize,
    min_cosine: f32,
) -> Result<Vec<SearchHit>> {
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
    use std::sync::OnceLock;

    static EMBEDDER: OnceLock<TextEmbedding> = OnceLock::new();
    let model_dir = std::env::var_os("FASTEMBED_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| wisp_dir.join("cache").join("models"));
    std::fs::create_dir_all(&model_dir).map_err(|e| WispError::internal(e.to_string()))?;
    let embedder = match EMBEDDER.get() {
        Some(e) => e,
        None => {
            let e = TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::BGESmallENV15)
                    .with_cache_dir(model_dir)
                    .with_show_download_progress(false),
            )
            .map_err(|e| WispError::internal(format!("semantic model init: {e}")))?;
            let _ = EMBEDDER.set(e);
            EMBEDDER.get().expect("embedder set above")
        }
    };

    sync_dense(conn, embedder, wisp_dir)?;
    let qv = embedder
        .embed(vec![query], None)
        .map_err(|e| WispError::internal(e.to_string()))?[0]
        .clone();

    let mut hits = Vec::new();
    let mut st = conn
        .prepare(
            "SELECT e.section, s.concept, c.path, s.heading, s.body, e.vector, e.dimensions
             FROM section_dense_embeddings e
             JOIN state_sections s ON s.section = e.section
             JOIN state_concepts c ON c.concept = s.concept
             WHERE e.model_tag = ?1",
        )
        .map_err(|e| WispError::internal(e.to_string()))?;
    let rows = st
        .query_map([MODEL_TAG], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Vec<u8>>(5)?,
                r.get::<_, i32>(6)?,
            ))
        })
        .map_err(|e| WispError::internal(e.to_string()))?;

    let floor = min_cosine as f64;
    for row in rows.filter_map(|r| r.ok()) {
        let (sec_b, con_b, path, heading, body, blob, dim) = row;
        let vec = blob_to_f32(&blob, dim as usize)?;
        let score = cosine(&qv, &vec);
        if score < floor {
            continue;
        }
        hits.push(SearchHit {
            concept: wisp_model::ConceptId::from_bytes(blob16(&con_b)),
            section: wisp_model::SectionId::from_bytes(blob16(&sec_b)),
            path,
            heading,
            score,
            body,
        });
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if hits.first().is_none_or(|h| h.score < floor) {
        return Ok(Vec::new());
    }
    hits.truncate(k);
    Ok(hits)
}

#[cfg(feature = "semantic")]
fn sync_dense(
    conn: &Connection,
    embedder: &fastembed::TextEmbedding,
    _wisp_dir: &Path,
) -> Result<()> {
    use wisp_store::Hot;
    for c in Hot::concepts(conn)? {
        for s in Hot::sections_of(conn, c.concept)? {
            if s.level == 0 {
                continue;
            }
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM section_dense_embeddings WHERE section = ?1 AND model_tag = ?2",
                    rusqlite::params![s.section.bytes().to_vec(), MODEL_TAG],
                    |r| r.get(0),
                )
                .map_err(|e| WispError::internal(e.to_string()))?;
            if n > 0 {
                continue;
            }
            let text = format!("{}\n{}", s.heading, s.body);
            let vec = embedder
                .embed(vec![text], None)
                .map_err(|e| WispError::internal(e.to_string()))?[0]
                .clone();
            let blob = f32_to_blob(&vec);
            conn.execute(
                "INSERT OR REPLACE INTO section_dense_embeddings (section, model_tag, dimensions, vector)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    s.section.bytes().to_vec(),
                    MODEL_TAG,
                    vec.len() as i32,
                    blob,
                ],
            )
            .map_err(|e| WispError::internal(e.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(not(feature = "semantic"))]
pub fn search_semantic(
    _wisp_dir: &Path,
    _conn: &Connection,
    _query: &str,
    _k: usize,
    _min_cosine: f32,
) -> Result<Vec<SearchHit>> {
    Err(WispError::usage(
        "semantic search requires feature `semantic` on wisp-sub-embed",
    ))
}

#[cfg(feature = "semantic")]
fn blob16(v: &[u8]) -> [u8; 16] {
    let mut b = [0u8; 16];
    if v.len() == 16 {
        b.copy_from_slice(v);
    }
    b
}

#[cfg(feature = "semantic")]
fn f32_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(feature = "semantic")]
fn blob_to_f32(blob: &[u8], dim: usize) -> Result<Vec<f32>> {
    if blob.len() != dim * 4 {
        return Err(WispError::internal("dense vector blob size mismatch"));
    }
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        let b = blob[i * 4..(i + 1) * 4].try_into().unwrap();
        out.push(f32::from_le_bytes(b));
    }
    Ok(out)
}

#[cfg(feature = "semantic")]
fn cosine(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() as f64
}

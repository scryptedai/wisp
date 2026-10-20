//! Postgres vector index backend (pgvector or `REAL[]` / BYTEA fallback).
//!
//! Build with `--features postgres` and set `url_env` in manifest (default `WISP_VECTOR_DSN`).

mod schema;

use wisp_index::{SectionVectorRow, VectorIndexBackend, VectorSearchHit};
use wisp_model::{ConceptId, Result, SectionId, WispError};

pub use schema::PGVECTOR_DDL;

pub fn open_from_env(codec_id: &str, url_env: &str) -> Result<Box<dyn VectorIndexBackend>> {
    let dsn = std::env::var(url_env)
        .map_err(|_| WispError::usage(format!("set {url_env} for postgres vector index")))?;
    open(codec_id, &dsn)
}

pub fn open(codec_id: &str, dsn: &str) -> Result<Box<dyn VectorIndexBackend>> {
    #[cfg(feature = "postgres")]
    {
        return Ok(Box::new(PostgresVectorBackend::connect(codec_id, dsn)?));
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = (codec_id, dsn);
        Err(WispError::not_implemented(
            "wisp-vector-pg: rebuild wisp-cli with --features postgres-vector",
        ))
    }
}

#[cfg(feature = "postgres")]
struct PostgresVectorBackend {
    codec: String,
    client: postgres::Client,
    dim: usize,
}

#[cfg(feature = "postgres")]
impl PostgresVectorBackend {
    fn connect(codec_id: &str, dsn: &str) -> Result<Self> {
        let client = postgres::Client::connect(dsn, postgres::NoTls)
            .map_err(|e| WispError::internal(format!("postgres connect: {e}")))?;
        client
            .batch_execute(schema::REAL_ARRAY_DDL)
            .map_err(|e| WispError::internal(format!("postgres schema: {e}")))?;
        Ok(PostgresVectorBackend {
            codec: codec_id.to_string(),
            client,
            dim: 64,
        })
    }

    fn vec_blob(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn blob_vec(blob: &[u8], dim: usize) -> Option<Vec<f32>> {
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
}

#[cfg(feature = "postgres")]
impl VectorIndexBackend for PostgresVectorBackend {
    fn codec_id(&self) -> &str {
        &self.codec
    }

    fn upsert(&mut self, rows: &[SectionVectorRow]) -> Result<()> {
        for r in rows {
            let blob = Self::vec_blob(&r.vector);
            self.client
                .execute(
                    "INSERT INTO wisp_vectors (codec, section, concept, version, path, heading, body, embedding)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                     ON CONFLICT (codec, section) DO UPDATE SET
                       concept = EXCLUDED.concept, version = EXCLUDED.version,
                       path = EXCLUDED.path, heading = EXCLUDED.heading, body = EXCLUDED.body,
                       embedding = EXCLUDED.embedding",
                    &[
                        &self.codec,
                        &r.section.bytes().to_vec(),
                        &r.concept.bytes().to_vec(),
                        &r.version.bytes().to_vec(),
                        &r.path,
                        &r.heading,
                        &r.body,
                        &blob,
                    ],
                )
                .map_err(|e| WispError::internal(e.to_string()))?;
        }
        Ok(())
    }

    fn delete_sections(&mut self, sections: &[SectionId]) -> Result<()> {
        for s in sections {
            self.client
                .execute(
                    "DELETE FROM wisp_vectors WHERE codec = $1 AND section = $2",
                    &[&self.codec, &s.bytes().to_vec()],
                )
                .map_err(|e| WispError::internal(e.to_string()))?;
        }
        Ok(())
    }

    fn replace_all(&mut self, rows: &[SectionVectorRow]) -> Result<()> {
        self.client
            .execute("DELETE FROM wisp_vectors WHERE codec = $1", &[&self.codec])
            .map_err(|e| WispError::internal(e.to_string()))?;
        self.upsert(rows)
    }

    fn search(&self, query: &[f32], k: usize) -> Result<Vec<VectorSearchHit>> {
        let rows = self
            .client
            .query(
                "SELECT concept, section, path, heading, body, embedding FROM wisp_vectors WHERE codec = $1",
                &[&self.codec],
            )
            .map_err(|e| WispError::internal(e.to_string()))?;
        let mut hits = Vec::new();
        for row in rows {
            let concept = ConceptId::from_slice(row.get::<_, Vec<u8>>(0))
                .ok_or_else(|| WispError::integrity("postgres vector row: bad concept id"))?;
            let section = SectionId::from_slice(row.get::<_, Vec<u8>>(1))
                .ok_or_else(|| WispError::integrity("postgres vector row: bad section id"))?;
            let blob: Vec<u8> = row.get(5);
            let Some(vec) = Self::blob_vec(&blob, self.dim) else {
                continue;
            };
            let score = Self::cosine(query, &vec);
            hits.push(VectorSearchHit {
                concept,
                section,
                path: row.get(2),
                heading: row.get(3),
                body: row.get(4),
                score,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }
}

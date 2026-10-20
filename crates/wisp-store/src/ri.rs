//! Random indexing (RI) with incremental TF-IDF in hot.db (§15).

use crate::hot::{SearchHit, SectionState};
use crate::sql_err;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use wisp_model::{ConceptId, Result, SectionId, WispError};

pub const RI_DIM: usize = 256;
/// ~2% non-zero dimensions per term (98% sparse).
const RI_NONZERO_PER_TERM: usize = 5;
pub const RI_MIN_COSINE: f64 = 0.12;
/// When lexical search returned nothing, require a stronger RI match (agent-safe).
pub const RI_STANDALONE_MIN_COSINE: f64 = 0.22;

pub fn schema_sql() -> &'static str {
    r"
CREATE TABLE IF NOT EXISTS ri_term_df (
  term TEXT PRIMARY KEY,
  df INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ri_section_terms (
  section BLOB NOT NULL,
  term TEXT NOT NULL,
  tf REAL NOT NULL,
  PRIMARY KEY (section, term)
);
CREATE TABLE IF NOT EXISTS ri_section_vectors (
  section BLOB PRIMARY KEY,
  dimensions INTEGER NOT NULL,
  vector BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS ri_meta (
  key TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);
"
}

fn corpus_sections(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT value FROM ri_meta WHERE key = 'n_sections'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(sql_err)?
        .unwrap_or(0))
}

fn set_corpus_sections(conn: &Connection, n: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO ri_meta (key, value) VALUES ('n_sections', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![n],
    )
    .map_err(sql_err)?;
    Ok(())
}

/// Unicode-aware tokens (min length 2) for RI / IDF.
pub fn terms(text: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    let mut current = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            current.push(ch);
        } else if !current.is_empty() {
            if current.len() >= 2 {
                *counts.entry(current.clone()).or_insert(0) += 1;
            }
            current.clear();
        }
    }
    if current.len() >= 2 {
        *counts.entry(current).or_insert(0) += 1;
    }
    counts
}

fn term_random_add(out: &mut [f32], term: &str, weight: f64) {
    if weight <= 0.0 {
        return;
    }
    let digest = Sha256::digest(term.as_bytes());
    let mut used = [false; RI_DIM];
    let mut placed = 0usize;
    let mut i = 0usize;
    while placed < RI_NONZERO_PER_TERM && i < digest.len() * 4 {
        let b0 = digest[i % digest.len()];
        let b1 = digest[(i + 1) % digest.len()];
        let idx = ((b0 as usize) << 4 ^ (b1 as usize)) % RI_DIM;
        if !used[idx] {
            used[idx] = true;
            let sign = if digest[(i + 2) % digest.len()] & 1 == 0 {
                1.0
            } else {
                -1.0
            };
            out[idx] += sign * weight as f32;
            placed += 1;
        }
        i += 1;
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn idf(n_sections: i64, df: i64) -> f64 {
    let n = n_sections.max(1) as f64;
    let d = df.max(0) as f64;
    ((n + 1.0) / (d + 1.0)).ln() + 1.0
}

fn tf_weight(count: u32) -> f64 {
    1.0 + (count as f64).ln()
}

pub fn unindex_section(conn: &Connection, section: SectionId) -> Result<()> {
    let sb = section.bytes().to_vec();
    let had: bool = conn
        .query_row(
            "SELECT 1 FROM ri_section_vectors WHERE section = ?1",
            params![sb],
            |_| Ok(true),
        )
        .optional()
        .map_err(sql_err)?
        .unwrap_or(false);
    if !had {
        return Ok(());
    }

    let mut st = conn
        .prepare("SELECT term FROM ri_section_terms WHERE section = ?1")
        .map_err(sql_err)?;
    let term_rows: Vec<String> = st
        .query_map(params![sb.clone()], |r| r.get(0))
        .map_err(sql_err)?
        .filter_map(|r| r.ok())
        .collect();

    for term in term_rows {
        let df: i64 = conn
            .query_row(
                "SELECT df FROM ri_term_df WHERE term = ?1",
                params![term],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let new_df = df - 1;
        if new_df <= 0 {
            conn.execute("DELETE FROM ri_term_df WHERE term = ?1", params![term])
                .map_err(sql_err)?;
        } else {
            conn.execute(
                "UPDATE ri_term_df SET df = ?2 WHERE term = ?1",
                params![term, new_df],
            )
            .map_err(sql_err)?;
        }
    }

    conn.execute(
        "DELETE FROM ri_section_terms WHERE section = ?1",
        params![sb],
    )
    .map_err(sql_err)?;
    conn.execute(
        "DELETE FROM ri_section_vectors WHERE section = ?1",
        params![sb],
    )
    .map_err(sql_err)?;

    let n = corpus_sections(conn)?;
    set_corpus_sections(conn, (n - 1).max(0))?;
    Ok(())
}

pub fn reindex_section(conn: &Connection, s: &SectionState) -> Result<()> {
    if s.level == 0 {
        return Ok(());
    }
    unindex_section(conn, s.section)?;

    let text = format!("{}\n{}", s.heading, s.body);
    let term_counts = terms(&text);
    if term_counts.is_empty() {
        return Ok(());
    }

    let sb = s.section.bytes().to_vec();
    for (term, count) in &term_counts {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM ri_term_df WHERE term = ?1",
                params![term],
                |_| Ok(true),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(false);
        if exists {
            conn.execute(
                "UPDATE ri_term_df SET df = df + 1 WHERE term = ?1",
                params![term],
            )
            .map_err(sql_err)?;
        } else {
            conn.execute(
                "INSERT INTO ri_term_df (term, df) VALUES (?1, 1)",
                params![term],
            )
            .map_err(sql_err)?;
        }
        conn.execute(
            "INSERT INTO ri_section_terms (section, term, tf) VALUES (?1, ?2, ?3)",
            params![sb.clone(), term, *count as f64],
        )
        .map_err(sql_err)?;
    }

    let n_sections = corpus_sections(conn)? + 1;
    set_corpus_sections(conn, n_sections)?;

    let mut vec = vec![0f32; RI_DIM];
    for (term, count) in &term_counts {
        let df: i64 = conn
            .query_row(
                "SELECT df FROM ri_term_df WHERE term = ?1",
                params![term],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let w = tf_weight(*count) * idf(n_sections, df);
        term_random_add(&mut vec, term, w);
    }
    l2_normalize(&mut vec);
    let blob = vec
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect::<Vec<u8>>();
    conn.execute(
        "INSERT INTO ri_section_vectors (section, dimensions, vector) VALUES (?1, ?2, ?3)",
        params![sb, RI_DIM as i32, blob],
    )
    .map_err(sql_err)?;
    Ok(())
}

pub fn ensure_indexed(conn: &Connection) -> Result<()> {
    let indexed: i64 = conn
        .query_row("SELECT COUNT(*) FROM ri_section_vectors", [], |r| r.get(0))
        .map_err(sql_err)?;
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_sections WHERE level > 0",
            [],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    if indexed >= live {
        return Ok(());
    }
    let mut st = conn
        .prepare(
            "SELECT concept, section, idx, level, heading, heading_line, body, scope, version
             FROM state_sections WHERE level > 0",
        )
        .map_err(sql_err)?;
    let rows = st
        .query_map([], |r| {
            Ok(SectionState {
                concept: ConceptId::from_bytes(crate::blob16(r.get::<_, Vec<u8>>(0)?)),
                section: SectionId::from_bytes(crate::blob16(r.get::<_, Vec<u8>>(1)?)),
                idx: r.get(2)?,
                level: r.get::<_, i64>(3)? as u8,
                heading: r.get(4)?,
                heading_line: r.get(5)?,
                body: r.get(6)?,
                scope: wisp_model::ScopeId::from_bytes(crate::blob16(r.get::<_, Vec<u8>>(7)?)),
                version: wisp_model::ChangeId::from_bytes(crate::blob16(r.get::<_, Vec<u8>>(8)?)),
            })
        })
        .map_err(sql_err)?;
    for row in rows.filter_map(|r| r.ok()) {
        if conn
            .query_row(
                "SELECT 1 FROM ri_section_vectors WHERE section = ?1",
                params![row.section.bytes().to_vec()],
                |_| Ok(true),
            )
            .optional()
            .map_err(sql_err)?
            .is_none()
        {
            reindex_section(conn, &row)?;
        }
    }
    Ok(())
}

fn query_vector(conn: &Connection, query: &str) -> Result<Vec<f32>> {
    let term_counts = terms(query);
    if term_counts.is_empty() {
        return Ok(vec![0f32; RI_DIM]);
    }
    let n_sections = corpus_sections(conn)?;
    let mut vec = vec![0f32; RI_DIM];
    for (term, count) in &term_counts {
        let df: i64 = conn
            .query_row(
                "SELECT df FROM ri_term_df WHERE term = ?1",
                params![term],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .unwrap_or(0);
        let w = tf_weight(*count) * idf(n_sections, df);
        term_random_add(&mut vec, term, w);
    }
    l2_normalize(&mut vec);
    Ok(vec)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum()
}

fn blob_to_vec(blob: &[u8], dim: usize) -> Result<Vec<f32>> {
    if blob.len() != dim * 4 {
        return Err(WispError::internal("ri vector blob size mismatch"));
    }
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        let b = blob[i * 4..(i + 1) * 4].try_into().unwrap();
        out.push(f32::from_le_bytes(b));
    }
    Ok(out)
}

pub fn search(
    conn: &Connection,
    query: &str,
    k: usize,
    min_score: f64,
    standalone_lexical_miss: bool,
) -> Result<Vec<SearchHit>> {
    ensure_indexed(conn)?;
    let qv = query_vector(conn, query)?;
    if qv.iter().all(|&x| x == 0.0) {
        return Ok(Vec::new());
    }
    let floor = if standalone_lexical_miss {
        RI_STANDALONE_MIN_COSINE.max(min_score)
    } else {
        min_score
    };

    let mut st = conn
        .prepare(
            "SELECT v.section, s.concept, c.path, s.heading, s.body, v.vector, v.dimensions
             FROM ri_section_vectors v
             JOIN state_sections s ON s.section = v.section
             JOIN state_concepts c ON c.concept = s.concept",
        )
        .map_err(sql_err)?;
    let mut hits = Vec::new();
    let rows = st
        .query_map([], |r| {
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
        .map_err(sql_err)?;

    for row in rows.filter_map(|r| r.ok()) {
        let (sec_b, con_b, path, heading, body, blob, dim) = row;
        let sv = blob_to_vec(&blob, dim as usize)?;
        let score = cosine(&qv, &sv);
        if score < floor {
            continue;
        }
        hits.push(SearchHit {
            concept: ConceptId::from_bytes(crate::blob16(con_b)),
            section: SectionId::from_bytes(crate::blob16(sec_b)),
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
    if hits.len() > 1 {
        let margin = hits[0].score - hits[1].score;
        if margin < 0.04 {
            return Ok(Vec::new());
        }
    }
    hits.truncate(k);
    Ok(hits)
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM ri_section_terms; DELETE FROM ri_section_vectors; DELETE FROM ri_term_df; DELETE FROM ri_meta;",
    )
    .map_err(sql_err)?;
    Ok(())
}

pub fn vector_rows(conn: &Connection, section: SectionId) -> Result<u64> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ri_section_vectors WHERE section = ?1",
            params![section.bytes().to_vec()],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    Ok(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hot::{Hot, SectionState};
    use tempfile::tempdir;
    use wisp_model::{ChangeId, ConceptId, ScopeId};

    #[test]
    fn ri_retracts_on_erase() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hot.db");
        let hot = Hot::open(&path).unwrap();
        let conn = hot.conn();
        let concept = ConceptId::new();
        let section = SectionId::new();
        let scope = ScopeId::new();
        conn.execute(
            "INSERT INTO state_concepts (concept, path, type, status, scope, version)
             VALUES (?1, 'doc.md', 'Note', 'stable', ?2, ?3)",
            params![
                concept.bytes().to_vec(),
                scope.bytes().to_vec(),
                ChangeId::new().bytes().to_vec()
            ],
        )
        .unwrap();
        let s = SectionState {
            concept,
            section,
            idx: 0,
            level: 1,
            heading: "OOM {#sec-oom}".into(),
            heading_line: "# OOM".into(),
            body: "out of memory killer".into(),
            scope,
            version: ChangeId::new(),
        };
        Hot::upsert_section(conn, &s).unwrap();
        assert_eq!(vector_rows(conn, section).unwrap(), 1);
        let df: i64 = conn
            .query_row("SELECT df FROM ri_term_df WHERE term = 'memory'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(df, 1);
        Hot::delete_section(conn, section).unwrap();
        assert_eq!(vector_rows(conn, section).unwrap(), 0);
        let n = conn
            .query_row(
                "SELECT COUNT(*) FROM ri_term_df WHERE term = 'memory'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }
}

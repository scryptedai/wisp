//! Tier 0: anchor (`#sec-…`) and quoted phrase fast path.

use serde_json::Value;
use wisp_model::Result;
use wisp_store::Hot;

pub fn search(conn: &rusqlite::Connection, query: &str, k: usize) -> Result<Vec<Value>> {
    let hits = Hot::search_exact(conn, query, k)?;
    Ok(hits.into_iter().map(crate::search::hit_json).collect())
}

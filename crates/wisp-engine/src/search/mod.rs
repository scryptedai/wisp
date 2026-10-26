//! Multi-tier search: exact → lexical/trigram → RI → optional neural.

mod exact;
mod lexical_rrf;

use crate::repo::Repository;
use serde_json::{json, Value};
use wisp_model::{Result, WispError};
use wisp_store::{Hot, SearchField, RI_MIN_COSINE, RI_STANDALONE_MIN_COSINE};

pub use lexical_rrf::rrf_merge_lexical;

/// Optional overrides from CLI (`--threshold`).
#[derive(Clone, Debug, Default)]
pub struct SearchParams {
    pub threshold: Option<f32>,
    /// Skip neural tier (no model load/download). Honored for `semantic` mode and `WISP_LOCAL_ONLY=1`.
    pub local_only: bool,
    /// Restrict match to one column (`heading`, `body`, `title`, `path`).
    pub field: Option<String>,
}

pub fn execute(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    mode: &str,
    k: usize,
    params: &SearchParams,
) -> Result<Value> {
    if params.field.is_some() {
        return field_search(repo, ref_name, query, mode, k, params);
    }
    let (mode_used, results) = match mode {
        "auto" => auto_search(repo, ref_name, query, k, params)?,
        "semantic" => semantic_search(repo, ref_name, query, k, params)?,
        other => {
            let hot = repo.hot(ref_name)?;
            let conn = hot.conn();
            match other {
                "exact" => {
                    let hits = exact::search(conn, query, k)?;
                    ("exact".to_string(), hits)
                }
                "keyword" => {
                    let hits = Hot::search_keyword(conn, query, k)?;
                    ("keyword".to_string(), lexical_json(hits))
                }
                "fuzzy" => {
                    let bm25 = Hot::search_keyword(conn, query, k)?;
                    let tri = Hot::search_char_trigram(conn, query, k, params.threshold)?;
                    let merged = rrf_merge_lexical(query, bm25, tri, k);
                    ("fuzzy".to_string(), merged)
                }
                "ri" => {
                    let floor = ri_floor(params);
                    let hits = Hot::search_ri(conn, query, k, floor, true)?;
                    ("ri".to_string(), lexical_json(hits))
                }
                unknown => {
                    return Err(WispError::usage(format!(
                        "unknown search mode {unknown:?}; use auto, exact, fuzzy, keyword, ri, or semantic"
                    )));
                }
            }
        }
    };

    Ok(search_envelope(mode_used, mode, query, params, results))
}

fn search_envelope(
    mode_used: String,
    requested_mode: &str,
    query: &str,
    params: &SearchParams,
    results: Vec<Value>,
) -> Value {
    let mut o = json!({
        "mode": mode_used,
        "requested_mode": requested_mode,
        "query": query,
        "local_only": params.local_only || local_only_from_env(),
        "results": results,
    });
    if let Some(f) = &params.field {
        o["field"] = json!(f);
    }
    o
}

fn field_search(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    mode: &str,
    k: usize,
    params: &SearchParams,
) -> Result<Value> {
    let field_name = params
        .field
        .as_deref()
        .ok_or_else(|| WispError::usage("field search requires --field"))?;
    let field = SearchField::parse(field_name).ok_or_else(|| {
        WispError::usage(format!(
            "unknown --field {field_name:?}; use heading, body, title, or path"
        ))
    })?;
    let hot = repo.hot(ref_name)?;
    let conn = hot.conn();
    let (mode_used, hits) = match mode {
        "auto" => {
            let exact = Hot::search_field(conn, field, query, "exact", k)?;
            if !exact.is_empty() {
                ("exact".to_string(), exact)
            } else {
                let kw = Hot::search_field(conn, field, query, "keyword", k)?;
                ("keyword".to_string(), kw)
            }
        }
        "exact" | "keyword" => {
            let hits = Hot::search_field(conn, field, query, mode, k)?;
            (mode.to_string(), hits)
        }
        other => {
            return Err(WispError::usage(format!(
                "search --field does not support mode {other:?}; use auto, exact, or keyword"
            )));
        }
    };
    let results = hits.into_iter().map(hit_json).collect();
    Ok(search_envelope(mode_used, mode, query, params, results))
}

fn auto_search(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    k: usize,
    params: &SearchParams,
) -> Result<(String, Vec<Value>)> {
    let hot = repo.hot(ref_name)?;
    let conn = hot.conn();
    let exact_hits = exact::search(conn, query, k)?;
    if !exact_hits.is_empty() {
        return Ok(("exact".to_string(), exact_hits));
    }
    let bm25 = Hot::search_keyword(conn, query, k)?;
    let bm25_weak = bm25.first().map(|h| h.score).unwrap_or(0.0) < 2.0;
    let tri_floor = if bm25.is_empty() || bm25_weak {
        params
            .threshold
            .or(Some(wisp_store::STANDALONE_CHAR_TRIGRAM_MIN))
    } else {
        params.threshold
    };
    let tri = Hot::search_char_trigram(conn, query, k, tri_floor)?;
    let merged = rrf_merge_lexical(query, bm25, tri, k);
    if !merged.is_empty() {
        return Ok(("auto".to_string(), merged));
    }
    let floor = ri_floor(params);
    let ri = Hot::search_ri(conn, query, k, floor, true)?;
    if !ri.is_empty() {
        return Ok(("ri".to_string(), lexical_json(ri)));
    }
    if !effective_local_only(params) && wisp_sub_embed::semantic_enabled() {
        if let Ok(v) = semantic_search(repo, ref_name, query, k, params) {
            return Ok(v);
        }
    }
    Ok(("auto".to_string(), Vec::new()))
}

fn ri_floor(params: &SearchParams) -> f64 {
    params
        .threshold
        .map(|t| t as f64)
        .unwrap_or(RI_STANDALONE_MIN_COSINE.max(RI_MIN_COSINE))
}

fn local_only_from_env() -> bool {
    std::env::var("WISP_LOCAL_ONLY")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn effective_local_only(params: &SearchParams) -> bool {
    params.local_only || local_only_from_env()
}

fn lexical_json(hits: Vec<wisp_store::SearchHit>) -> Vec<Value> {
    hits.into_iter().map(hit_json).collect()
}

pub(crate) fn hit_json(h: wisp_store::SearchHit) -> Value {
    let mut o = json!({
        "concept": h.concept.to_string(),
        "path": h.path,
        "section": h.section.to_string(),
        "heading": h.heading,
        "score": h.score,
        "tokens": crate::query::count_tokens(&h.body),
    });
    if let Some(v) = h.version {
        o["version"] = json!(v.to_string());
    }
    if let Some(f) = h.matched_field {
        o["field"] = json!(f);
    }
    if let Some(s) = h.snippet {
        o["snippet"] = json!(s);
    }
    o
}

fn semantic_search(
    repo: &mut Repository,
    ref_name: &str,
    query: &str,
    k: usize,
    params: &SearchParams,
) -> Result<(String, Vec<Value>)> {
    if effective_local_only(params) {
        return Err(WispError::usage(
            "semantic search is disabled in local-only mode (omit --local-only or unset WISP_LOCAL_ONLY)",
        ));
    }
    if !wisp_sub_embed::semantic_enabled() {
        return Err(WispError::usage(
            "semantic search requires building wisp with feature `semantic` (rebuild without --no-default-features)",
        ));
    }
    let wisp_dir = repo.wisp_dir.clone();
    let hot = repo.hot(ref_name)?;
    let floor = params
        .threshold
        .unwrap_or(wisp_sub_embed::NEURAL_MIN_COSINE);
    let hits = wisp_sub_embed::search_semantic(wisp_dir.as_path(), hot.conn(), query, k, floor)?;
    let results = hits.into_iter().map(hit_json).collect();
    Ok(("semantic".to_string(), results))
}

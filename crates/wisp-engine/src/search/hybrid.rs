//! Reciprocal rank fusion for lexical + fuzzy legs.

use serde_json::{json, Value};
use std::collections::HashMap;
use wisp_store::SearchHit;

use super::hit_json;

pub fn rrf_merge_lexical(
    query: &str,
    bm25: Vec<SearchHit>,
    fuzzy: Vec<SearchHit>,
    k: usize,
) -> Vec<Value> {
    let bm25_sections: std::collections::HashSet<String> =
        bm25.iter().map(|h| h.section.to_string()).collect();
    let alone_min = wisp_store::STANDALONE_CHAR_TRIGRAM_MIN as f64;
    let fuzzy: Vec<SearchHit> = fuzzy
        .into_iter()
        .filter(|h| bm25_sections.contains(&h.section.to_string()) || h.score >= alone_min)
        .collect();
    let mut scores: HashMap<String, (f64, SearchHit)> = HashMap::new();
    for (i, h) in bm25.into_iter().enumerate() {
        let key = h.section.to_string();
        let rrf = 1.0 / (60.0 + i as f64);
        scores.insert(key, (rrf, h));
    }
    for (i, h) in fuzzy.into_iter().enumerate() {
        let key = h.section.to_string();
        let rrf = 1.0 / (60.0 + i as f64);
        if let Some((s, existing)) = scores.get_mut(&key) {
            *s += rrf;
            if h.score > existing.score {
                *existing = h;
            }
        } else {
            scores.insert(key, (rrf, h));
        }
    }
    let mut ranked: Vec<_> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1 .0
            .partial_cmp(&a.1 .0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let term_count = query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|t| t.len() >= 3)
        .count();
    let mut ranked: Vec<(f64, SearchHit)> = ranked
        .into_iter()
        .map(|(_, (score, h))| (score, h))
        .filter(|(_, h)| {
            let in_bm25 = bm25_sections.contains(&h.section.to_string());
            let need = min_lexical_token_hits(query);
            lexical_token_hits(query, &h) >= need || (!in_bm25 && h.score >= alone_min)
        })
        .collect();
    if term_count >= 4 {
        let best_lex = ranked
            .iter()
            .map(|(_, h)| lexical_token_hits(query, h))
            .max()
            .unwrap_or(0);
        if best_lex < 3 {
            ranked.clear();
        }
    }
    ranked
        .into_iter()
        .take(k)
        .map(|(score, h)| {
            let mut o = hit_json(h);
            o.as_object_mut()
                .unwrap()
                .insert("score".into(), json!(score));
            o
        })
        .collect()
}

fn min_lexical_token_hits(query: &str) -> usize {
    let n = query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|t| t.len() >= 3)
        .count();
    match n {
        0 => 0,
        1..=3 => 1,
        4..=5 => 3,
        _ => (n + 1) / 2,
    }
}

fn lexical_token_hits(query: &str, hit: &SearchHit) -> usize {
    let text = format!("{}\n{}", hit.heading, hit.body).to_lowercase();
    query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|t| t.len() >= 3)
        .filter(|t| text.contains(&t.to_lowercase()))
        .count()
}

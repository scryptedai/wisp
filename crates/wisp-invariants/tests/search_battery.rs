//! Search IR battery: trigram/lexical + RI + semantic.
//!
//! **Canonical probes:** `contrib/search-battery/cases.toml` (also used by `run_battery.py`).

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use wisp_engine::query::{search, search_with_params};
use wisp_engine::repo::{Unlock, MAIN_REF};
use wisp_engine::search::SearchParams;
use wisp_invariants::{catalog, init_and_record_bundle};

const CASES_TOML: &str = include_str!("../../../contrib/search-battery/cases.toml");

#[derive(Clone, Copy, PartialEq)]
enum Expect {
    Top1,
    InTopK,
    AbsentTopK,
    Informative,
}

#[derive(Clone)]
struct Case {
    id: String,
    query: String,
    anchor: String,
    category: String,
    expect: Expect,
    keyword_in_top_k: bool,
    auto_semantic_top1: bool,
}

#[derive(Deserialize)]
struct CasesFile {
    k: usize,
    cases: Vec<CaseRow>,
}

#[derive(Deserialize)]
struct CaseRow {
    id: String,
    query: String,
    anchor: String,
    category: String,
    expect: String,
    keyword_in_top_k: bool,
    auto_semantic_top1: bool,
}

fn parse_expect(s: &str) -> Expect {
    match s {
        "top1" => Expect::Top1,
        "topK" => Expect::InTopK,
        "miss@K" => Expect::AbsentTopK,
        "report" => Expect::Informative,
        other => panic!("unknown expect in cases.toml: {other}"),
    }
}

fn load_cases() -> (usize, Vec<Case>) {
    let raw: CasesFile = toml::from_str(CASES_TOML).expect("parse cases.toml");
    let cases = raw
        .cases
        .into_iter()
        .map(|r| Case {
            id: r.id,
            query: r.query,
            anchor: r.anchor,
            category: r.category,
            expect: parse_expect(&r.expect),
            keyword_in_top_k: r.keyword_in_top_k,
            auto_semantic_top1: r.auto_semantic_top1,
        })
        .collect();
    (raw.k, cases)
}

fn assert_absent_top_k(c: &Case, mode: &str, k: usize, rank: Option<usize>, results: &[Value]) {
    assert!(
        rank.is_none(),
        "{} [{}] expected miss for {} in top {}, rank {:?}",
        c.id,
        mode,
        c.anchor,
        k,
        rank
    );
    if mode == "auto" || mode == "fuzzy" {
        assert!(
            results.is_empty(),
            "{} [{}] expected empty results (no ghost hits), got {}",
            c.id,
            mode,
            results.len()
        );
    }
}

fn hit_anchor(hit: &Value, anchor: &str) -> bool {
    let heading = hit["heading"].as_str().unwrap_or("");
    let path = hit["path"].as_str().unwrap_or("");
    heading.contains(anchor) || path.contains(anchor)
}

fn ranks_for_query(results: &[Value], anchor: &str) -> Vec<usize> {
    results
        .iter()
        .enumerate()
        .filter(|(_, h)| hit_anchor(h, anchor))
        .map(|(i, _)| i + 1)
        .collect()
}

struct CatStats {
    total: usize,
    hit_at_1: usize,
    hit_at_k: usize,
    mrr_sum: f64,
}

impl CatStats {
    fn new() -> Self {
        Self {
            total: 0,
            hit_at_1: 0,
            hit_at_k: 0,
            mrr_sum: 0.0,
        }
    }

    fn record_hit(&mut self, rank: Option<usize>) {
        self.total += 1;
        if let Some(r) = rank {
            self.hit_at_k += 1;
            if r == 1 {
                self.hit_at_1 += 1;
            }
            self.mrr_sum += 1.0 / r as f64;
        }
    }
}

fn expect_label(c: &Case, mode: &str, neural: bool) -> String {
    if c.expect == Expect::AbsentTopK
        && c.auto_semantic_top1
        && neural
        && (mode == "auto" || mode == "semantic")
    {
        return "top1@neural".to_string();
    }
    expect_label_base(c.expect)
}

fn expect_label_base(e: Expect) -> String {
    match e {
        Expect::Top1 => "top1".to_string(),
        Expect::InTopK => "topK".to_string(),
        Expect::AbsentTopK => "miss@K".to_string(),
        Expect::Informative => "report".to_string(),
    }
}

fn run_mode(
    repo: &mut wisp_engine::repo::Repository,
    mode: &str,
    k: usize,
    cases: &[Case],
    neural: bool,
) -> HashMap<String, CatStats> {
    let mut by_cat: HashMap<String, CatStats> = HashMap::new();
    println!("\n=== mode: {mode} ===");
    println!(
        "{:<8} {:<22} {:<12} {:<6} {:<8} top path",
        "id", "category", "expect", "rank", "score@1"
    );
    for c in cases {
        let v = search(repo, MAIN_REF, &c.query, mode, k).expect("search");
        let results = v["results"].as_array().cloned().unwrap_or_default();
        let ranks = ranks_for_query(&results, &c.anchor);
        let rank = ranks.first().copied();
        let score1 = results
            .first()
            .and_then(|h| h["score"].as_f64())
            .unwrap_or(0.0);
        let top_path = results
            .first()
            .and_then(|h| h["path"].as_str())
            .unwrap_or("-");
        let rank_s = rank
            .map(|r| r.to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "{:<8} {:<22} {:<12} {:<6} {:<8.4} {}",
            c.id,
            c.category,
            expect_label(c, mode, neural),
            rank_s,
            score1,
            top_path
        );
        if !results.is_empty() && c.id == "T4.1" {
            let oom_ranks = ranks_for_query(&results, "sec-oom");
            if let Some(r) = oom_ranks.first() {
                println!("         (sec-oom also in top-{k} at rank {r})");
            }
        }
        let stats = by_cat
            .entry(c.category.clone())
            .or_insert_with(CatStats::new);
        if c.expect != Expect::Informative {
            stats.record_hit(rank);
        }
        if !should_assert(mode, c) {
            continue;
        }
        match c.expect {
            Expect::Top1 => assert_eq!(
                rank,
                Some(1),
                "{} [{}] expected rank 1 for anchor {}",
                c.id,
                mode,
                c.anchor
            ),
            Expect::InTopK => assert!(
                rank.is_some(),
                "{} [{}] expected anchor {} in top {}, got: {:?}",
                c.id,
                mode,
                c.anchor,
                k,
                results
                    .iter()
                    .map(|h| format!("{}:{}", h["path"], h["heading"]))
                    .collect::<Vec<_>>()
            ),
            Expect::AbsentTopK => {
                if neural && c.auto_semantic_top1 && (mode == "auto" || mode == "semantic") {
                    assert_eq!(
                        rank,
                        Some(1),
                        "{} [{}] expected rank 1 for {} (neural)",
                        c.id,
                        mode,
                        c.anchor
                    );
                    if mode == "auto" {
                        assert_eq!(
                            v["mode"].as_str(),
                            Some("semantic"),
                            "{} [auto] expected mode semantic",
                            c.id
                        );
                    }
                } else {
                    assert_absent_top_k(c, mode, k, rank, &results);
                }
            }
            Expect::Informative => {}
        }
    }
    by_cat
}

fn should_assert(mode: &str, c: &Case) -> bool {
    match c.expect {
        Expect::Top1 => mode != "ri",
        Expect::InTopK => match mode {
            "keyword" => c.keyword_in_top_k,
            "ri" => false,
            "auto" | "fuzzy" => true,
            _ => false,
        },
        Expect::AbsentTopK => true,
        Expect::Informative => false,
    }
}

fn print_summary(mode: &str, k: usize, by_cat: &HashMap<String, CatStats>) {
    println!("\n--- Search evaluation summary ({mode}) ---");
    let mut cats: Vec<_> = by_cat.keys().collect();
    cats.sort();
    for cat in cats {
        let s = &by_cat[cat];
        if s.total == 0 {
            continue;
        }
        let n = s.total as f64;
        println!("[{}] (N={})", cat.to_uppercase(), s.total);
        println!("  Recall@1: {:.2}%", 100.0 * s.hit_at_1 as f64 / n);
        println!("  Recall@{}: {:.2}%", k, 100.0 * s.hit_at_k as f64 / n);
        println!("  MRR:      {:.3}", s.mrr_sum / n);
    }
}

#[test]
fn search_battery_modes() {
    let (k, cases) = load_cases();
    let cat = catalog();
    let spec = cat
        .bundle("search-battery")
        .expect("search-battery bundle in catalog");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo =
        init_and_record_bundle(spec, &root, &Unlock::default()).expect("init search-battery");

    let neural = cfg!(feature = "semantic");

    for mode in ["auto", "fuzzy", "keyword", "ri"] {
        let stats = run_mode(&mut repo, mode, k, &cases, neural);
        print_summary(mode, k, &stats);
    }

    if neural {
        let stats = run_mode(&mut repo, "semantic", k, &cases, true);
        print_summary("semantic", k, &stats);

        let params = SearchParams {
            threshold: None,
            local_only: true,
        };
        for c in cases.iter().filter(|c| c.expect == Expect::AbsentTopK) {
            let v = search_with_params(&mut repo, MAIN_REF, &c.query, "auto", k, &params)
                .expect("local-only auto");
            let results = v["results"].as_array().cloned().unwrap_or_default();
            assert!(
                ranks_for_query(&results, &c.anchor).is_empty(),
                "{} [auto local_only] expected miss for {}",
                c.id,
                c.anchor
            );
        }
    }
}

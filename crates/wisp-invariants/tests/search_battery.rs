//! Search IR battery: BM25 + `emb:hash:64` hybrid (see contrib/search-battery/README.md).
//!
//! **Semantic-gap probes (T3.x)** must miss at @5 on keyword, vector, and hybrid so agents get
//! empty results instead of RRF ghost hits. **T2.1** (zero token overlap typos) is report-only:
//! `emb:hash:64` is bag-of-token hashing, not character n-gram fuzzy match.

use serde_json::Value;
use std::collections::HashMap;
use wisp_engine::query::search;
use wisp_engine::repo::MAIN_REF;
use wisp_engine::repo::Unlock;
use wisp_invariants::{catalog, init_and_record_bundle};

const K: usize = 5;

#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// Must rank 1 in top-K.
    Top1,
    /// Must appear somewhere in top-K.
    InTopK,
    /// Must not appear in top-K (semantic gap).
    AbsentTopK,
    /// Log only (polysemy / partial-hit probes).
    Informative,
}

struct Case {
    id: &'static str,
    query: &'static str,
    anchor: &'static str,
    category: &'static str,
    expect: Expect,
    /// If false, `InTopK` is not asserted under keyword mode (e.g. deliberate typos).
    keyword_in_top_k: bool,
    note: &'static str,
}

const CASES: &[Case] = &[
    Case {
        id: "T1.0",
        query: "database failover standby replica",
        anchor: "sec-failover",
        category: "lexical",
        expect: Expect::Top1,
        keyword_in_top_k: true,
        note: "BM25 baseline",
    },
    Case {
        id: "T2.1",
        query: "databse replca hearbeat",
        anchor: "sec-failover",
        category: "syntactic_fuzzy",
        expect: Expect::Informative,
        keyword_in_top_k: false,
        note: "BM25 miss; hash-64 has no shared tokens (was a ghost hit before cosine floor)",
    },
    Case {
        id: "T2.2a",
        query: "out of memory",
        anchor: "sec-oom",
        category: "syntactic",
        expect: Expect::InTopK,
        keyword_in_top_k: true,
        note: "FTS tokenizer (spaced)",
    },
    Case {
        id: "T2.2b",
        query: "out-of-memory",
        anchor: "sec-oom",
        category: "syntactic",
        expect: Expect::InTopK,
        keyword_in_top_k: true,
        note: "FTS tokenizer (hyphenated)",
    },
    Case {
        id: "T3.1",
        query: "RAM depleted process terminated",
        anchor: "sec-oom",
        category: "semantic_gap",
        expect: Expect::AbsentTopK,
        keyword_in_top_k: true,
        note: "no lexical overlap with OOM section",
    },
    Case {
        id: "T3.2",
        query: "GDPR right to be forgotten compliance",
        anchor: "sec-crypto-shred",
        category: "semantic_gap",
        expect: Expect::AbsentTopK,
        keyword_in_top_k: true,
        note: "paraphrase without shared tokens",
    },
    Case {
        id: "T4.1",
        query: "capital starvation bank liquidity",
        anchor: "sec-liquidity",
        category: "polysemy",
        expect: Expect::Informative,
        keyword_in_top_k: true,
        note: "watch false pull to sec-oom (starvation)",
    },
    Case {
        id: "T4.2",
        query: "operating system thread context switch",
        anchor: "sec-trading-pipeline",
        category: "polysemy",
        expect: Expect::Informative,
        keyword_in_top_k: true,
        note: "OS terms in trading doc",
    },
    Case {
        id: "T5.1",
        query: "OOM kernel kill",
        anchor: "sec-oom",
        category: "acronym",
        expect: Expect::Informative,
        keyword_in_top_k: true,
        note: "partial token overlap",
    },
];

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

fn run_mode(
    repo: &mut wisp_engine::repo::Repository,
    mode: &str,
    cases: &[Case],
) -> HashMap<String, CatStats> {
    let mut by_cat: HashMap<String, CatStats> = HashMap::new();
    println!("\n=== mode: {} ===", mode);
    println!(
        "{:<8} {:<22} {:<12} {:<6} {:<8} {}",
        "id", "category", "expect", "rank", "score@1", "top path"
    );
    for c in cases {
        let v = search(repo, MAIN_REF, c.query, mode, K).expect("search");
        let results = v["results"].as_array().cloned().unwrap_or_default();
        let ranks = ranks_for_query(&results, c.anchor);
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
            expect_label(c.expect),
            rank_s,
            score1,
            top_path
        );
        if !results.is_empty() && c.id == "T4.1" {
            let oom_ranks = ranks_for_query(&results, "sec-oom");
            if let Some(r) = oom_ranks.first() {
                println!("         (sec-oom also in top-{} at rank {})", K, r);
            }
        }
        let stats = by_cat.entry(c.category.to_string()).or_insert_with(CatStats::new);
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
                K,
                results
                    .iter()
                    .map(|h| format!("{}:{}", h["path"], h["heading"]))
                    .collect::<Vec<_>>()
            ),
            Expect::AbsentTopK => {
                assert!(
                    rank.is_none(),
                    "{} [{}] expected miss for {} in top {}, rank {:?}",
                    c.id,
                    mode,
                    c.anchor,
                    K,
                    rank
                );
                if mode == "vector" || mode == "hybrid" {
                    assert!(
                        results.is_empty(),
                        "{} [{}] semantic gap should return no hits (agent-safe empty), got {}",
                        c.id,
                        mode,
                        results.len()
                    );
                }
            }
            Expect::Informative => {}
        }
    }
    by_cat
}

fn should_assert(mode: &str, c: &Case) -> bool {
    match c.expect {
        // `emb:hash:64` vector-only scores are coarse; lexical @1 is gated on hybrid + keyword.
        Expect::Top1 => mode != "vector",
        Expect::InTopK => match mode {
            "keyword" => c.keyword_in_top_k,
            "hybrid" => true,
            _ => false,
        },
        // No ghost hits on unrelated queries in any search mode.
        Expect::AbsentTopK => true,
        Expect::Informative => false,
    }
}

fn expect_label(e: Expect) -> &'static str {
    match e {
        Expect::Top1 => "top1",
        Expect::InTopK => "topK",
        Expect::AbsentTopK => "miss@K",
        Expect::Informative => "report",
    }
}

fn print_summary(mode: &str, by_cat: &HashMap<String, CatStats>) {
    println!("\n--- Search evaluation summary ({}) ---", mode);
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
        println!("  Recall@{}: {:.2}%", K, 100.0 * s.hit_at_k as f64 / n);
        println!("  MRR:      {:.3}", s.mrr_sum / n);
    }
}

#[test]
fn search_battery_hybrid_keyword_vector() {
    let cat = catalog();
    let spec = cat
        .bundle("search-battery")
        .expect("search-battery bundle in catalog");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bundle");
    let mut repo = init_and_record_bundle(spec, &root, &Unlock::default())
        .expect("init search-battery");

    for mode in ["hybrid", "keyword", "vector"] {
        let stats = run_mode(&mut repo, mode, CASES);
        print_summary(mode, &stats);
    }
}

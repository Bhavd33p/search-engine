//! Retrieval-quality metrics over a labelled query set.
//!
//! For each query the engine returns a ranked list; human judgements say which
//! documents are relevant (and how much). Each metric is averaged over queries.
//!
//! * Recall@k — fraction of the relevant documents found in the top k.
//! * Average precision (AP) — mean of precision@r over the ranks r where a
//!   relevant document appears; rewards putting relevant documents early.
//!   MAP is AP averaged over queries.
//! * nDCG@k — discounted cumulative gain of the top k (graded relevance,
//!   divided by log2(rank + 1)), normalised by the best possible ordering.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::cranfield::{CranfieldQuery, Judgements, gain, relevant_docs};
use crate::engine::{Engine, QueryMode, SearchError};
use crate::index::store::StoreError;
use crate::rank::RankerKind;

pub fn recall_at(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let found = ranked.iter().take(k).filter(|doc| relevant.contains(*doc)).count();
    found as f64 / relevant.len() as f64
}

pub fn average_precision(ranked: &[String], relevant: &HashSet<String>) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let mut found = 0;
    let mut precision_sum = 0.0;
    for (index, doc) in ranked.iter().enumerate() {
        if relevant.contains(doc) {
            found += 1;
            precision_sum += found as f64 / (index + 1) as f64;
        }
    }
    precision_sum / relevant.len() as f64
}

pub fn ndcg_at(ranked: &[String], gains: &HashMap<String, f64>, k: usize) -> f64 {
    let dcg = |gains_in_rank_order: &mut dyn Iterator<Item = f64>| -> f64 {
        gains_in_rank_order
            .take(k)
            .enumerate()
            .map(|(index, gain)| gain / ((index + 2) as f64).log2())
            .sum()
    };
    let actual = dcg(&mut ranked.iter().map(|doc| gains.get(doc).copied().unwrap_or(0.0)));
    let mut ideal_gains: Vec<f64> = gains.values().copied().collect();
    ideal_gains.sort_by(|a, b| b.total_cmp(a));
    let ideal = dcg(&mut ideal_gains.into_iter());
    if ideal == 0.0 { 0.0 } else { actual / ideal }
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub ranker: RankerKind,
    pub queries: usize,
    pub recall_at_10: f64,
    pub recall_at_100: f64,
    pub map: f64,
    pub ndcg_at_10: f64,
}

/// Runs every query as free text (OR of its words, ranked) and averages the
/// metrics. `depth` is how many results are retrieved per query for MAP.
pub fn evaluate_cranfield(
    engine: &Engine,
    queries: &[CranfieldQuery],
    judgements: &Judgements,
    ranker: RankerKind,
    depth: usize,
) -> Result<EvalReport, SearchError> {
    let external_ids = external_ids(engine)?;
    let (mut recall_10, mut recall_100, mut map, mut ndcg_10) = (0.0, 0.0, 0.0, 0.0);
    for query in queries {
        let ranked: Vec<String> = match engine.analyze(&query.text, QueryMode::AnyTerms) {
            Ok(analyzed) => engine
                .rank(&analyzed, depth, ranker)?
                .docs
                .iter()
                .map(|scored| external_ids[scored.doc_id as usize].clone())
                .collect(),
            Err(SearchError::NoSearchableTerms) => Vec::new(),
            Err(error) => return Err(error),
        };
        let relevant = relevant_docs(judgements, query.id);
        let gains: HashMap<String, f64> = judgements
            .get(&query.id)
            .map(|docs| docs.iter().map(|(doc, &grade)| (doc.clone(), gain(grade))).collect())
            .unwrap_or_default();
        recall_10 += recall_at(&ranked, &relevant, 10);
        recall_100 += recall_at(&ranked, &relevant, 100);
        map += average_precision(&ranked, &relevant);
        ndcg_10 += ndcg_at(&ranked, &gains, 10);
    }
    let n = queries.len().max(1) as f64;
    Ok(EvalReport {
        ranker,
        queries: queries.len(),
        recall_at_10: recall_10 / n,
        recall_at_100: recall_100 / n,
        map: map / n,
        ndcg_at_10: ndcg_10 / n,
    })
}

fn external_ids(engine: &Engine) -> Result<Vec<String>, StoreError> {
    (0..engine.stats().num_docs)
        .map(|doc_id| {
            engine
                .reader()
                .document(doc_id)?
                .map(|doc| doc.id)
                .ok_or(StoreError::Missing("a stored document"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn recall_counts_relevant_docs_in_the_top_k() {
        let ranked = docs(&["a", "x", "b", "y", "c"]);
        let relevant = set(&["a", "b", "c", "d"]);
        assert_close(recall_at(&ranked, &relevant, 1), 0.25);
        assert_close(recall_at(&ranked, &relevant, 3), 0.5);
        assert_close(recall_at(&ranked, &relevant, 100), 0.75);
        assert_close(recall_at(&ranked, &set(&[]), 10), 0.0);
    }

    #[test]
    fn average_precision_rewards_early_hits() {
        // Relevant at ranks 1 and 3, one relevant never retrieved:
        // (1/1 + 2/3) / 3 = 0.5556
        let ranked = docs(&["a", "x", "b"]);
        assert_close(
            average_precision(&ranked, &set(&["a", "b", "c"])),
            (1.0 + 2.0 / 3.0) / 3.0,
        );
        assert_close(average_precision(&docs(&["a", "b"]), &set(&["a", "b"])), 1.0);
        assert_close(average_precision(&docs(&["x", "a"]), &set(&["a"])), 0.5);
    }

    #[test]
    fn ndcg_is_one_for_the_ideal_order_and_lower_otherwise() {
        let gains: HashMap<String, f64> = [("a".to_string(), 3.0), ("b".to_string(), 1.0)].into();
        assert_close(ndcg_at(&docs(&["a", "b"]), &gains, 10), 1.0);
        // Swapped: (1/log2 2 + 3/log2 3) / (3/log2 2 + 1/log2 3)
        let expected = (1.0 + 3.0 / 3f64.log2()) / (3.0 + 1.0 / 3f64.log2());
        assert_close(ndcg_at(&docs(&["b", "a"]), &gains, 10), expected);
        assert_close(ndcg_at(&docs(&["x"]), &gains, 10), 0.0);
    }

    #[test]
    fn ndcg_only_looks_at_the_top_k() {
        let gains: HashMap<String, f64> = [("a".to_string(), 1.0)].into();
        assert_close(ndcg_at(&docs(&["x", "a"]), &gains, 1), 0.0);
    }
}

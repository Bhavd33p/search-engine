//! The "no index" baseline: answer a query by scanning every document.
//!
//! For each query it re-analyses every document (tokenise, stop words, stem),
//! checks the boolean/phrase structure against that document's positions, and
//! counts term frequencies — then scores the matches with the same BM25 code
//! the engine uses. It returns exactly the engine's results, so benchmarks
//! compare like with like, and tests use it as an independent oracle.
//!
//! Only BM25 is supported: TF-IDF's per-document norms need statistics for
//! every term in the collection, which a single scan cannot provide.

use std::collections::HashMap;

use crate::analysis::Analyzer;
use crate::corpus::Document;
use crate::query::AnalyzedQuery;
use crate::rank::topk::{ScoredDoc, top_k};
use crate::rank::{Bm25, CollectionStats, DocTermStats, Scorer, TermStats};

pub struct LinearScan<'a> {
    analyzer: &'a Analyzer,
    documents: &'a [Document],
    bm25: Bm25,
}

/// A matching document's length and the frequency of each scoring term in it.
struct Match {
    doc_id: u32,
    doc_length: u32,
    term_frequencies: Vec<u32>,
}

impl<'a> LinearScan<'a> {
    pub fn new(analyzer: &'a Analyzer, documents: &'a [Document], bm25: Bm25) -> Self {
        Self {
            analyzer,
            documents,
            bm25,
        }
    }

    pub fn search(&self, query: &AnalyzedQuery, k: usize) -> Vec<ScoredDoc> {
        let terms: Vec<(String, u32)> = query.scoring_terms().into_iter().collect();
        let mut document_frequencies = vec![0u32; terms.len()];
        let mut total_length: u64 = 0;
        let mut matches = Vec::new();

        for (doc_id, document) in self.documents.iter().enumerate() {
            let tokens = self.analyzer.analyze_document(&document.title, &document.body);
            total_length += tokens.len() as u64;
            let mut positions: HashMap<&str, Vec<u32>> = HashMap::new();
            for token in &tokens {
                positions.entry(token.term.as_str()).or_default().push(token.position);
            }
            let term_frequencies: Vec<u32> = terms
                .iter()
                .map(|(term, _)| positions.get(term.as_str()).map_or(0, |p| p.len() as u32))
                .collect();
            for (df, &tf) in document_frequencies.iter_mut().zip(&term_frequencies) {
                if tf > 0 {
                    *df += 1;
                }
            }
            if matches_document(query, &positions) {
                matches.push(Match {
                    doc_id: doc_id as u32,
                    doc_length: tokens.len() as u32,
                    term_frequencies,
                });
            }
        }

        let num_docs = self.documents.len() as u32;
        let avg_doc_len = if num_docs == 0 {
            0.0
        } else {
            total_length as f64 / f64::from(num_docs)
        };
        let collection = CollectionStats { num_docs, avg_doc_len };
        let scored = matches.into_iter().map(|m| {
            let mut score = 0.0;
            for (index, (_, query_frequency)) in terms.iter().enumerate() {
                if m.term_frequencies[index] == 0 {
                    continue;
                }
                let term = TermStats {
                    document_frequency: document_frequencies[index],
                    query_frequency: *query_frequency,
                };
                let doc = DocTermStats {
                    term_frequency: m.term_frequencies[index],
                    doc_length: m.doc_length,
                    doc_norm: 0.0,
                };
                score += self.bm25.score(&collection, &term, &doc);
            }
            ScoredDoc {
                doc_id: m.doc_id,
                score,
            }
        });
        top_k(scored, k)
    }
}

/// Evaluates the query against one document's term positions.
fn matches_document(query: &AnalyzedQuery, positions: &HashMap<&str, Vec<u32>>) -> bool {
    match query {
        AnalyzedQuery::Term(term) => positions.contains_key(term.as_str()),
        AnalyzedQuery::Phrase(parts) => {
            let Some((first, rest)) = parts.split_first() else {
                return false;
            };
            let Some(starts) = positions.get(first.term.as_str()) else {
                return false;
            };
            starts.iter().any(|&start| {
                rest.iter().all(|part| {
                    let expected = (start + part.offset).checked_sub(first.offset);
                    positions
                        .get(part.term.as_str())
                        .is_some_and(|found| expected.is_some_and(|position| found.binary_search(&position).is_ok()))
                })
            })
        }
        AnalyzedQuery::And(children) => children.iter().all(|child| matches_document(child, positions)),
        AnalyzedQuery::Or(children) => children.iter().any(|child| matches_document(child, positions)),
        AnalyzedQuery::Not(inner) => !matches_document(inner, positions),
    }
}

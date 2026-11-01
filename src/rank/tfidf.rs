//! TF-IDF with log term frequency and cosine (length) normalisation.
//!
//! ```text
//! idf(t)      = ln(N / df)
//! w(t, d)     = (1 + ln tf) · idf(t)            document term weight
//! w(t, q)     = (1 + ln qtf) · idf(t)           query term weight
//! |d|         = sqrt(Σ_t w(t, d)²)              over every term in d
//! score(q, d) = Σ_t w(t, q) · w(t, d) / |d|
//! ```
//!
//! This is the cosine similarity between the query and document vectors
//! (SMART "ltc.ltc"), except that we skip dividing by the query's own length:
//! it is the same for every document, so it never changes the ranking.
//!
//! `|d|` needs every term of the document, not just the query terms, so it is
//! computed once at index time ([`document_norms`]) and stored in the index.

use super::{CollectionStats, DocTermStats, Scorer, TermStats};
use crate::index::builder::InMemoryIndex;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TfIdf;

impl TfIdf {
    pub fn idf(num_docs: u32, document_frequency: u32) -> f64 {
        if document_frequency == 0 {
            return 0.0;
        }
        (f64::from(num_docs) / f64::from(document_frequency)).ln()
    }

    /// `(1 + ln tf) · idf`, or 0 when the term is absent.
    pub fn weight(frequency: u32, idf: f64) -> f64 {
        if frequency == 0 {
            0.0
        } else {
            (1.0 + f64::from(frequency).ln()) * idf
        }
    }
}

impl Scorer for TfIdf {
    fn score(&self, collection: &CollectionStats, term: &TermStats, doc: &DocTermStats) -> f64 {
        if doc.doc_norm == 0.0 {
            return 0.0;
        }
        let idf = Self::idf(collection.num_docs, term.document_frequency);
        Self::weight(term.query_frequency, idf) * Self::weight(doc.term_frequency, idf) / doc.doc_norm
    }
}

/// Length of every document's TF-IDF vector, indexed by doc id.
pub fn document_norms(index: &InMemoryIndex) -> Vec<f64> {
    let mut squared = vec![0.0; index.documents.len()];
    for (_, list) in &index.terms {
        let idf = TfIdf::idf(index.num_docs(), list.document_frequency());
        for posting in list.postings() {
            let weight = TfIdf::weight(posting.term_frequency(), idf);
            squared[posting.doc_id as usize] += weight * weight;
        }
    }
    squared.into_iter().map(f64::sqrt).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analyzer;
    use crate::corpus::Document;

    fn example_index() -> InMemoryIndex {
        let bodies = ["rust is fast", "rust rust borrow check", "safe code is fast code"];
        let docs = bodies.iter().enumerate().map(|(i, body)| Document {
            id: i.to_string(),
            url: String::new(),
            title: String::new(),
            body: body.to_string(),
        });
        InMemoryIndex::build(&Analyzer::new(), docs)
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn norms_match_hand_computation() {
        let norms = document_norms(&example_index());
        // D0: sqrt(2 · ln(1.5)²); D1: sqrt(((1 + ln 2) ln 1.5)² + 2 · ln(3)²); D2: see WALKTHROUGH.md
        assert_close(norms[0], 0.573_414);
        assert_close(norms[1], 1.698_587);
        assert_close(norms[2], 2.198_037);
    }

    #[test]
    fn scores_match_hand_computed_example() {
        let norms = document_norms(&example_index());
        let collection = CollectionStats {
            num_docs: 3,
            avg_doc_len: 10.0 / 3.0,
        };
        let term = TermStats {
            document_frequency: 2,
            query_frequency: 1,
        };
        let score = |tf, doc_id: usize| {
            TfIdf.score(
                &collection,
                &term,
                &DocTermStats {
                    term_frequency: tf,
                    doc_length: 0,
                    doc_norm: norms[doc_id],
                },
            )
        };
        assert_close(score(1, 0) * 2.0, 0.573_414); // rust + fast
        assert_close(score(2, 1), 0.163_875); // rust ×2
        assert_close(score(1, 2), 0.074_795); // fast
    }

    #[test]
    fn a_term_in_every_document_has_no_weight() {
        assert_eq!(TfIdf::idf(5, 5), 0.0);
    }

    #[test]
    fn empty_document_scores_zero_instead_of_dividing_by_zero() {
        let collection = CollectionStats {
            num_docs: 3,
            avg_doc_len: 1.0,
        };
        let term = TermStats {
            document_frequency: 1,
            query_frequency: 1,
        };
        let doc = DocTermStats {
            term_frequency: 1,
            doc_length: 1,
            doc_norm: 0.0,
        };
        assert_eq!(TfIdf.score(&collection, &term, &doc), 0.0);
    }
}

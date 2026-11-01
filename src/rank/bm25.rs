//! Okapi BM25.
//!
//! ```text
//! idf(t)    = ln(1 + (N - df + 0.5) / (df + 0.5))
//! score(t)  = qtf · idf(t) · tf · (k1 + 1) / (tf + k1 · (1 - b + b · dl / avgdl))
//! ```
//!
//! * `k1` controls term-frequency saturation: the 10th occurrence of a word adds
//!   far less than the 1st. `k1 = 0` ignores tf entirely.
//! * `b` controls length normalisation: `b = 1` fully penalises long documents,
//!   `b = 0` ignores length.
//!
//! The `1 +` inside the logarithm (as in Lucene) keeps idf positive even for
//! terms that appear in more than half of the documents.

use serde::{Deserialize, Serialize};

use super::{CollectionStats, DocTermStats, Scorer, TermStats};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bm25 {
    pub k1: f64,
    pub b: f64,
}

impl Default for Bm25 {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

impl Bm25 {
    pub fn idf(num_docs: u32, document_frequency: u32) -> f64 {
        let n = f64::from(num_docs);
        let df = f64::from(document_frequency);
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }
}

impl Scorer for Bm25 {
    fn score(&self, collection: &CollectionStats, term: &TermStats, doc: &DocTermStats) -> f64 {
        let tf = f64::from(doc.term_frequency);
        if tf == 0.0 {
            return 0.0;
        }
        let length_ratio = if collection.avg_doc_len > 0.0 {
            f64::from(doc.doc_length) / collection.avg_doc_len
        } else {
            1.0
        };
        let saturation = tf * (self.k1 + 1.0) / (tf + self.k1 * (1.0 - self.b + self.b * length_ratio));
        f64::from(term.query_frequency) * Self::idf(collection.num_docs, term.document_frequency) * saturation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The 3-document example worked by hand in docs/WALKTHROUGH.md:
    //   D0 "rust is fast"           -> rust, fast              (dl 2)
    //   D1 "rust rust borrow check" -> rust×2, borrow, check   (dl 4)
    //   D2 "safe code is fast code" -> safe, code×2, fast      (dl 4)
    // N = 3, avgdl = 10/3, query "rust fast", df(rust) = df(fast) = 2.
    const COLLECTION: CollectionStats = CollectionStats {
        num_docs: 3,
        avg_doc_len: 10.0 / 3.0,
    };
    const TERM: TermStats = TermStats {
        document_frequency: 2,
        query_frequency: 1,
    };

    fn doc(term_frequency: u32, doc_length: u32) -> DocTermStats {
        DocTermStats {
            term_frequency,
            doc_length,
            doc_norm: 0.0,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn idf_matches_hand_computation() {
        // ln(1 + 1.5 / 2.5) = ln(1.6)
        assert_close(Bm25::idf(3, 2), 0.470_004);
    }

    #[test]
    fn scores_match_hand_computed_example() {
        let bm25 = Bm25::default();
        let d0 = 2.0 * bm25.score(&COLLECTION, &TERM, &doc(1, 2)); // rust and fast, tf 1 each
        let d1 = bm25.score(&COLLECTION, &TERM, &doc(2, 4)); // rust tf 2
        let d2 = bm25.score(&COLLECTION, &TERM, &doc(1, 4)); // fast tf 1
        assert_close(d0, 1.123_922);
        assert_close(d1, 0.611_839);
        assert_close(d2, 0.434_457);
    }

    #[test]
    fn term_frequency_saturates() {
        let bm25 = Bm25::default();
        let gain = |tf| bm25.score(&COLLECTION, &TERM, &doc(tf, 4)) - bm25.score(&COLLECTION, &TERM, &doc(tf - 1, 4));
        assert!(gain(2) < gain(1));
        assert!(gain(10) < gain(2));
        // Upper bound as tf → ∞ is idf · (k1 + 1).
        assert!(bm25.score(&COLLECTION, &TERM, &doc(1_000_000, 4)) < Bm25::idf(3, 2) * 2.2);
    }

    #[test]
    fn longer_documents_score_lower_for_the_same_tf() {
        let bm25 = Bm25::default();
        assert!(bm25.score(&COLLECTION, &TERM, &doc(1, 2)) > bm25.score(&COLLECTION, &TERM, &doc(1, 8)));
        let no_length_norm = Bm25 {
            b: 0.0,
            ..Bm25::default()
        };
        assert_close(
            no_length_norm.score(&COLLECTION, &TERM, &doc(1, 2)),
            no_length_norm.score(&COLLECTION, &TERM, &doc(1, 8)),
        );
    }

    #[test]
    fn idf_stays_positive_for_very_common_terms() {
        assert!(Bm25::idf(100, 100) > 0.0);
        assert!(Bm25::idf(100, 1) > Bm25::idf(100, 50));
    }
}

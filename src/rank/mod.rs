//! Relevance ranking, implemented from the formulas (no IR library).
//!
//! A query's score for a document is the sum of per-term scores. Each ranker
//! turns a handful of statistics into a per-term score:
//!
//! * collection: number of documents `N`, average document length `avgdl`;
//! * term: document frequency `df`, frequency in the query `qtf`;
//! * document: term frequency `tf`, document length `dl`, TF-IDF vector norm.

pub mod bm25;

pub use bm25::Bm25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollectionStats {
    pub num_docs: u32,
    pub avg_doc_len: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TermStats {
    /// Documents containing the term.
    pub document_frequency: u32,
    /// Times the term appears in the query.
    pub query_frequency: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocTermStats {
    /// Times the term appears in the document.
    pub term_frequency: u32,
    /// Indexed terms in the document.
    pub doc_length: u32,
    /// Euclidean length of the document's TF-IDF vector (used by TF-IDF only).
    pub doc_norm: f64,
}

/// Scores one query term against one document.
///
/// Implemented by [`Bm25`]. The search loop is generic over this
/// trait, so each ranker gets its own compiled copy of the loop (static dispatch).
pub trait Scorer {
    fn score(&self, collection: &CollectionStats, term: &TermStats, doc: &DocTermStats) -> f64;
}

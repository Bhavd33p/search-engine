//! Relevance ranking, implemented from the formulas (no IR library).
//!
//! A query's score for a document is the sum of per-term scores. Each ranker
//! turns a handful of statistics into a per-term score:
//!
//! * collection: number of documents `N`, average document length `avgdl`;
//! * term: document frequency `df`, frequency in the query `qtf`;
//! * document: term frequency `tf`, document length `dl`, TF-IDF vector norm.

pub mod bm25;
pub mod tfidf;
pub mod topk;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use bm25::Bm25;
pub use tfidf::TfIdf;

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
/// Implemented by [`Bm25`] and [`TfIdf`]. The search loop is generic over this
/// trait, so each ranker gets its own compiled copy of the loop (static dispatch).
pub trait Scorer {
    fn score(&self, collection: &CollectionStats, term: &TermStats, doc: &DocTermStats) -> f64;
}

/// Which ranking function a query uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum RankerKind {
    #[value(name = "bm25")]
    Bm25,
    #[value(name = "tfidf")]
    TfIdf,
}

impl fmt::Display for RankerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RankerKind::Bm25 => "bm25",
            RankerKind::TfIdf => "tfidf",
        })
    }
}

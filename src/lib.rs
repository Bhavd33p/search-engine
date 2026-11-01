//! A full-text search engine built from first principles.
//!
//! Pipeline: crawl → analyse (tokenise, stop words, stem) → invert into a
//! compressed positional index stored in RocksDB → parse queries → rank with
//! BM25 or TF-IDF → serve over gRPC.

pub mod analysis;
pub mod baseline;
pub mod cache;
pub mod corpus;
pub mod cranfield;
pub mod crawler;
pub mod engine;
pub mod eval;
pub mod grpc;
pub mod index;
pub mod query;
pub mod rank;
pub mod service;
pub mod snippet;

//! A full-text search engine built from first principles.
//!
//! Pipeline: crawl → analyse (tokenise, stop words, stem) → invert into a
//! compressed positional index stored in RocksDB → parse queries → rank with
//! BM25 or TF-IDF → serve over gRPC.

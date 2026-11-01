//! Caches that keep repeated work off the query path.
//!
//! * [`hot_terms`]: decoded posting lists of frequently used terms, in process memory.

pub mod hot_terms;

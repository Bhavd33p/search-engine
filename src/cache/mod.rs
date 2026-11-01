//! Caches that keep repeated work off the query path.
//!
//! * [`hot_terms`]: decoded posting lists of frequently used terms, in process memory;
//! * [`result_cache`]: complete query results, in Redis.

pub mod hot_terms;
pub mod result_cache;

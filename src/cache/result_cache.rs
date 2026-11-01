//! Caches complete query results in Redis, shared by every engine process.
//!
//! Key: `{namespace}:{index_version}:{ranker}:{k}:{canonical query}`.
//! * the index version changes whenever the index is rebuilt, so stale results
//!   are never served — old keys simply expire;
//! * the canonical query is the *analysed* query with AND/OR operands sorted,
//!   so "Foxes quick" and "quick fox" share an entry.
//!
//! Redis is an optimisation, never a dependency: if it is down at startup the
//! cache is disabled, and if a command fails later the query is answered from
//! the index as if it were a miss.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use redis::AsyncCommands;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use serde::Serialize;
use tracing::{info, warn};

use crate::engine::SearchResults;

/// Redis round trips slower than this count as failures, so a sick Redis
/// cannot make queries slower than having no cache at all.
const COMMAND_TIMEOUT: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ResultCacheConfig {
    pub url: String,
    pub ttl: Duration,
    /// Key prefix, so several deployments (or test runs) can share one Redis.
    pub namespace: String,
}

pub struct ResultCache {
    connection: ConnectionManager,
    config: ResultCacheConfig,
    hits: AtomicU64,
    misses: AtomicU64,
    errors: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResultCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub errors: u64,
}

impl ResultCache {
    /// Connects to Redis, or returns `None` (with a warning) when it is unreachable.
    pub async fn connect(config: ResultCacheConfig) -> Option<Self> {
        let client = match redis::Client::open(config.url.as_str()) {
            Ok(client) => client,
            Err(error) => {
                warn!(%error, url = %config.url, "invalid Redis URL; result cache disabled");
                return None;
            }
        };
        let manager_config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(CONNECT_TIMEOUT))
            .set_response_timeout(Some(COMMAND_TIMEOUT))
            .set_number_of_retries(1)
            .set_max_delay(Duration::from_secs(1));
        let connect = ConnectionManager::new_with_config(client, manager_config);
        match tokio::time::timeout(CONNECT_TIMEOUT * 2, connect).await {
            Ok(Ok(connection)) => {
                info!(url = %config.url, ttl_secs = config.ttl.as_secs(), "result cache enabled");
                Some(Self {
                    connection,
                    config,
                    hits: AtomicU64::new(0),
                    misses: AtomicU64::new(0),
                    errors: AtomicU64::new(0),
                })
            }
            Ok(Err(error)) => {
                warn!(%error, url = %config.url, "Redis unreachable; result cache disabled");
                None
            }
            Err(_) => {
                warn!(url = %config.url, "Redis connection timed out; result cache disabled");
                None
            }
        }
    }

    pub fn key(&self, index_version: &str, ranker: &str, k: usize, canonical_query: &str) -> String {
        format!(
            "{}:{index_version}:{ranker}:{k}:{canonical_query}",
            self.config.namespace
        )
    }

    /// Cached results for `key`; `None` on a miss *or* any Redis failure.
    pub async fn get(&self, key: &str) -> Option<SearchResults> {
        let mut connection = self.connection.clone();
        match connection.get::<_, Option<String>>(key).await {
            Ok(Some(json)) => match serde_json::from_str(&json) {
                Ok(results) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(results)
                }
                Err(error) => {
                    self.record_error("decode", &error);
                    None
                }
            },
            Ok(None) => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
            Err(error) => {
                self.record_error("GET", &error);
                None
            }
        }
    }

    pub async fn put(&self, key: &str, results: &SearchResults) {
        let json = match serde_json::to_string(results) {
            Ok(json) => json,
            Err(error) => return self.record_error("encode", &error),
        };
        let mut connection = self.connection.clone();
        if let Err(error) = connection
            .set_ex::<_, _, ()>(key, json, self.config.ttl.as_secs().max(1))
            .await
        {
            self.record_error("SET", &error);
        }
    }

    pub fn stats(&self) -> ResultCacheStats {
        ResultCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }

    fn record_error(&self, operation: &str, error: &dyn std::fmt::Display) {
        let count = self.errors.fetch_add(1, Ordering::Relaxed) + 1;
        // Log the 1st, 2nd, 4th, 8th… failure so an outage cannot flood the logs.
        if count.is_power_of_two() {
            warn!(operation, %error, failures = count, "result cache error; answering from the index");
        }
    }
}

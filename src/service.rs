//! The search service used by the CLI, the gRPC server and the benchmarks:
//! the engine plus the optional Redis result cache.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

use crate::cache::result_cache::ResultCache;
use crate::engine::{Engine, QueryMode, SearchError, SearchResults};
use crate::rank::RankerKind;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Search(#[from] SearchError),
    #[error("search task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl ServiceError {
    /// True when the caller sent a bad query, as opposed to a server fault.
    pub fn is_invalid_query(&self) -> bool {
        matches!(
            self,
            Self::Search(SearchError::Parse(_) | SearchError::NoSearchableTerms)
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceResponse {
    pub results: SearchResults,
    pub cache_hit: bool,
    pub took: Duration,
}

pub struct SearchService {
    engine: Arc<Engine>,
    cache: Option<ResultCache>,
}

impl SearchService {
    pub fn new(engine: Arc<Engine>, cache: Option<ResultCache>) -> Self {
        Self { engine, cache }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn cache(&self) -> Option<&ResultCache> {
        self.cache.as_ref()
    }

    pub async fn search(
        &self,
        text: &str,
        mode: QueryMode,
        k: usize,
        ranker: RankerKind,
    ) -> Result<ServiceResponse, ServiceError> {
        let started = Instant::now();
        let query = self.engine.analyze(text, mode)?;

        let key = self.cache.as_ref().map(|cache| {
            cache.key(
                &self.engine.stats().index_version,
                &self.engine.ranker_key(ranker),
                k,
                &query.canonical(),
            )
        });
        if let (Some(cache), Some(key)) = (&self.cache, &key)
            && let Some(results) = cache.get(key).await
        {
            return Ok(ServiceResponse {
                results,
                cache_hit: true,
                took: started.elapsed(),
            });
        }

        // RocksDB reads and scoring block the thread; keep them off the async workers.
        let engine = Arc::clone(&self.engine);
        let results = tokio::task::spawn_blocking(move || engine.search(&query, k, ranker)).await??;

        if let (Some(cache), Some(key)) = (&self.cache, &key) {
            cache.put(key, &results).await;
        }
        Ok(ServiceResponse {
            results,
            cache_hit: false,
            took: started.elapsed(),
        })
    }
}

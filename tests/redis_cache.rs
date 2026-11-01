//! Result-cache behaviour. Tests that need a live Redis read its URL from
//! `REDIS_URL` (CI starts a Redis service container) and are skipped without it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use search_engine::analysis::Analyzer;
use search_engine::cache::result_cache::{ResultCache, ResultCacheConfig};
use search_engine::corpus::Document;
use search_engine::engine::{Engine, EngineConfig, QueryMode};
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store::write_index;
use search_engine::rank::RankerKind;
use search_engine::service::SearchService;
use tempfile::TempDir;

fn engine() -> (TempDir, Arc<Engine>) {
    let docs = [
        ("1", "Ownership", "Each value in Rust has an owner."),
        ("2", "Borrowing", "References borrow values without taking ownership."),
        ("3", "Unsafe Rust", "Unsafe code can dereference raw pointers."),
    ]
    .map(|(id, title, body)| Document {
        id: id.into(),
        url: String::new(),
        title: title.into(),
        body: body.into(),
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    write_index(&path, &InMemoryIndex::build(&Analyzer::new(), docs), Codec::VByte).unwrap();
    let engine = Engine::open(&path, EngineConfig::default()).unwrap();
    (dir, Arc::new(engine))
}

/// A fresh key prefix per test run, so earlier runs' entries never leak in.
fn unique_namespace() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("se-test-{}-{nanos}", std::process::id())
}

fn config(url: &str) -> ResultCacheConfig {
    ResultCacheConfig {
        url: url.into(),
        ttl: Duration::from_secs(60),
        namespace: unique_namespace(),
    }
}

#[tokio::test]
async fn unreachable_redis_disables_the_cache_and_search_still_works() {
    // Port 1 is never a Redis server.
    let cache = ResultCache::connect(config("redis://127.0.0.1:1")).await;
    assert!(cache.is_none());

    let (_dir, engine) = engine();
    let service = SearchService::new(engine, cache);
    let response = service
        .search("rust", QueryMode::Boolean, 10, RankerKind::Bm25)
        .await
        .unwrap();
    assert!(!response.cache_hit);
    assert_eq!(response.results.total_matches, 2);
}

#[tokio::test]
async fn invalid_redis_url_disables_the_cache() {
    assert!(ResultCache::connect(config("not a url")).await.is_none());
}

#[tokio::test]
async fn second_identical_query_is_served_from_redis() {
    let Ok(url) = std::env::var("REDIS_URL") else {
        eprintln!("REDIS_URL not set; skipping");
        return;
    };
    let cache = ResultCache::connect(config(&url))
        .await
        .expect("REDIS_URL is set but Redis is unreachable");
    let (_dir, engine) = engine();
    let service = SearchService::new(engine, Some(cache));

    let first = service
        .search("rust ownership", QueryMode::Boolean, 10, RankerKind::Bm25)
        .await
        .unwrap();
    assert!(!first.cache_hit);

    // Same analysed query, different spelling and operand order.
    let second = service
        .search("Ownership AND Rust", QueryMode::Boolean, 10, RankerKind::Bm25)
        .await
        .unwrap();
    assert!(second.cache_hit);
    assert_eq!(second.results, first.results);

    // A different ranker or k is a different cache entry.
    let tfidf = service
        .search("rust ownership", QueryMode::Boolean, 10, RankerKind::TfIdf)
        .await
        .unwrap();
    assert!(!tfidf.cache_hit);
    let top1 = service
        .search("rust ownership", QueryMode::Boolean, 1, RankerKind::Bm25)
        .await
        .unwrap();
    assert!(!top1.cache_hit);

    let stats = service.cache().unwrap().stats();
    assert_eq!((stats.hits, stats.misses, stats.errors), (1, 3, 0));
}

#[tokio::test]
async fn cached_entries_expire() {
    let Ok(url) = std::env::var("REDIS_URL") else {
        eprintln!("REDIS_URL not set; skipping");
        return;
    };
    let config = config(&url);
    let namespace = config.namespace.clone();
    let cache = ResultCache::connect(config)
        .await
        .expect("REDIS_URL is set but Redis is unreachable");
    let (_dir, engine) = engine();
    let service = SearchService::new(engine, Some(cache));
    service
        .search("rust", QueryMode::Boolean, 10, RankerKind::Bm25)
        .await
        .unwrap();

    let client = redis::Client::open(url.as_str()).unwrap();
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{namespace}:*"))
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    let ttl: i64 = redis::cmd("TTL")
        .arg(&keys[0])
        .query_async(&mut connection)
        .await
        .unwrap();
    assert!((1..=60).contains(&ttl), "ttl {ttl}");
}

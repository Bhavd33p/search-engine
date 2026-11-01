//! Runs the gRPC server on an ephemeral port and talks to it with the generated client.

use std::sync::Arc;

use search_engine::analysis::Analyzer;
use search_engine::corpus::Document;
use search_engine::engine::{Engine, EngineConfig};
use search_engine::grpc::pb::search_api_client::SearchApiClient;
use search_engine::grpc::pb::{Ranker, SearchRequest, StatsRequest};
use search_engine::grpc::serve;
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store::write_index;
use search_engine::service::SearchService;
use tokio::sync::oneshot;
use tonic::Code;

#[tokio::test]
async fn search_stats_errors_and_graceful_shutdown() {
    let docs = [
        ("1", "Ownership", "Each value in Rust has an owner."),
        ("2", "Borrowing", "References borrow values without taking ownership."),
    ]
    .map(|(id, title, body)| Document {
        id: id.into(),
        url: format!("https://example.org/{id}"),
        title: title.into(),
        body: body.into(),
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    write_index(&path, &InMemoryIndex::build(&Analyzer::new(), docs), Codec::VByte).unwrap();
    let engine = Engine::open(&path, EngineConfig::default()).unwrap();
    let service = Arc::new(SearchService::new(Arc::new(engine), None));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, service, async {
        let _ = stopped.await;
    }));

    let mut client = SearchApiClient::connect(address).await.unwrap();

    let response = client
        .search(SearchRequest {
            query: "ownership".into(),
            k: 0,
            ranker: Ranker::Bm25.into(),
            any_terms: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.total_matches, 2);
    assert_eq!(response.hits[0].title, "Ownership", "title match ranks first");
    assert!(!response.cache_hit);

    let tfidf = client
        .search(SearchRequest {
            query: "owner".into(),
            k: 1,
            ranker: Ranker::Tfidf.into(),
            any_terms: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(tfidf.hits.len(), 1);

    let error = client
        .search(SearchRequest {
            query: "(ownership".into(),
            k: 5,
            ranker: 0,
            any_terms: false,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);

    let error = client
        .search(SearchRequest {
            query: "ownership".into(),
            k: 5,
            ranker: 99,
            any_terms: false,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);

    let stats = client.stats(StatsRequest {}).await.unwrap().into_inner();
    assert_eq!(stats.num_docs, 2);
    assert!(!stats.result_cache_enabled);
    assert!(stats.hot_term_misses > 0);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

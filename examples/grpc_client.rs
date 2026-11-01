//! Minimal gRPC client.
//!
//! ```sh
//! cargo run --release -- serve --index data/rust-docs-index &
//! cargo run --example grpc_client -- 'ownership "borrow checker"'
//! cargo run --example grpc_client -- --tfidf --any 'what does the borrow checker do'
//! ```

use anyhow::Result;
use search_engine::grpc::pb::search_api_client::SearchApiClient;
use search_engine::grpc::pb::{Ranker, SearchRequest, StatsRequest};

#[tokio::main]
async fn main() -> Result<()> {
    let mut ranker = Ranker::Bm25;
    let mut any_terms = false;
    let mut words = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--tfidf" => ranker = Ranker::Tfidf,
            "--any" => any_terms = true,
            _ => words.push(arg),
        }
    }
    let query = if words.is_empty() {
        "ownership".to_string()
    } else {
        words.join(" ")
    };
    let address = std::env::var("SEARCH_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let mut client = SearchApiClient::connect(address).await?;

    let stats = client.stats(StatsRequest {}).await?.into_inner();
    println!(
        "index: {} docs, {} terms, version {}",
        stats.num_docs, stats.num_terms, stats.index_version
    );

    let request = SearchRequest {
        query: query.clone(),
        k: 5,
        ranker: ranker.into(),
        any_terms,
    };
    let response = client.search(request).await?.into_inner();
    println!(
        "{query:?}: {} matches, {} µs, cache hit: {}",
        response.total_matches, response.took_micros, response.cache_hit
    );
    for (rank, hit) in response.hits.iter().enumerate() {
        println!("{:>2}. [{:.3}] {} <{}>", rank + 1, hit.score, hit.title, hit.url);
        println!("    {}", hit.snippet);
    }
    Ok(())
}

//! Builds a real on-disk index and searches it through the public API.

use search_engine::analysis::Analyzer;
use search_engine::corpus::Document;
use search_engine::engine::{Engine, EngineConfig, QueryMode};
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store::write_index;
use search_engine::rank::RankerKind;
use tempfile::TempDir;

fn doc(id: &str, title: &str, body: &str) -> Document {
    Document {
        id: id.into(),
        url: format!("https://example.org/{id}"),
        title: title.into(),
        body: body.into(),
    }
}

fn build(docs: Vec<Document>, codec: Codec) -> (TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    write_index(&path, &InMemoryIndex::build(&Analyzer::new(), docs), codec).unwrap();
    let engine = Engine::open(&path, EngineConfig::default()).unwrap();
    (dir, engine)
}

/// The 3-document example from docs/WALKTHROUGH.md.
fn walkthrough_corpus() -> Vec<Document> {
    vec![
        doc("d0", "", "rust is fast"),
        doc("d1", "", "rust rust borrow check"),
        doc("d2", "", "safe code is fast code"),
    ]
}

fn ids_and_scores(engine: &Engine, query: &str, mode: QueryMode, ranker: RankerKind) -> Vec<(String, f64)> {
    let analyzed = engine.analyze(query, mode).unwrap();
    let results = engine.search(&analyzed, 10, ranker).unwrap();
    results
        .hits
        .into_iter()
        .map(|hit| (hit.external_id, hit.score))
        .collect()
}

fn assert_ranking(actual: &[(String, f64)], expected: &[(&str, f64)]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?}");
    for ((id, score), (expected_id, expected_score)) in actual.iter().zip(expected) {
        assert_eq!(id, expected_id, "{actual:?}");
        assert!(
            (score - expected_score).abs() < 1e-6,
            "{id}: {score} != {expected_score}"
        );
    }
}

#[test]
fn bm25_ranking_matches_the_hand_computed_example() {
    for codec in [Codec::VByte, Codec::FixedU32] {
        let (_dir, engine) = build(walkthrough_corpus(), codec);
        let ranking = ids_and_scores(&engine, "rust fast", QueryMode::AnyTerms, RankerKind::Bm25);
        assert_ranking(&ranking, &[("d0", 1.123_922), ("d1", 0.611_839), ("d2", 0.434_457)]);
    }
}

#[test]
fn tfidf_ranking_matches_the_hand_computed_example() {
    let (_dir, engine) = build(walkthrough_corpus(), Codec::VByte);
    let ranking = ids_and_scores(&engine, "rust fast", QueryMode::AnyTerms, RankerKind::TfIdf);
    assert_ranking(&ranking, &[("d0", 0.573_414), ("d1", 0.163_875), ("d2", 0.074_795)]);
}

#[test]
fn boolean_mode_requires_every_term() {
    let (_dir, engine) = build(walkthrough_corpus(), Codec::VByte);
    let ranking = ids_and_scores(&engine, "rust fast", QueryMode::Boolean, RankerKind::Bm25);
    assert_ranking(&ranking, &[("d0", 1.123_922)]);
}

fn rust_corpus() -> Vec<Document> {
    vec![
        doc(
            "own",
            "Ownership",
            "Each value in Rust has an owner. When the owner goes out of scope, the value is dropped.",
        ),
        doc(
            "borrow",
            "References and Borrowing",
            "A reference lets you borrow a value without taking ownership of it.",
        ),
        doc(
            "unsafe",
            "Unsafe Rust",
            "Unsafe Rust lets you dereference raw pointers and opt out of memory safety checks.",
        ),
        doc(
            "gc",
            "Garbage Collection",
            "Many languages use a garbage collector to provide memory safety at runtime.",
        ),
        doc(
            "life",
            "Lifetimes",
            "Lifetimes ensure that references are valid as long as we need them to be.",
        ),
    ]
}

fn ids(engine: &Engine, query: &str) -> Vec<String> {
    let analyzed = engine.analyze(query, QueryMode::Boolean).unwrap();
    let mut ids: Vec<String> = engine
        .search(&analyzed, 10, RankerKind::Bm25)
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.external_id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn phrase_and_boolean_queries_end_to_end() {
    let (_dir, engine) = build(rust_corpus(), Codec::VByte);
    assert_eq!(ids(&engine, "\"memory safety\""), ["gc", "unsafe"]);
    assert_eq!(ids(&engine, "\"memory safety\" -unsafe"), ["gc"]);
    assert_eq!(ids(&engine, "rust"), ["own", "unsafe"]);
    assert_eq!(ids(&engine, "references OR ownership"), ["borrow", "life", "own"]);
    assert_eq!(ids(&engine, "(reference OR owner) -lifetimes"), ["borrow", "own"]);
    assert_eq!(ids(&engine, "rust AND garbage"), Vec::<String>::new());
}

#[test]
fn hits_carry_stored_fields_and_highlighted_snippets() {
    let (_dir, engine) = build(rust_corpus(), Codec::VByte);
    let query = engine.analyze("garbage", QueryMode::Boolean).unwrap();
    let results = engine.search(&query, 10, RankerKind::Bm25).unwrap();
    assert_eq!(results.total_matches, 1);
    let hit = &results.hits[0];
    assert_eq!(hit.title, "Garbage Collection");
    assert_eq!(hit.url, "https://example.org/gc");
    assert!(hit.snippet.contains("**garbage**"), "{}", hit.snippet);
}

#[test]
fn top_k_limits_hits_but_not_total_matches() {
    let (_dir, engine) = build(rust_corpus(), Codec::VByte);
    let query = engine
        .analyze("rust OR reference OR memory", QueryMode::Boolean)
        .unwrap();
    let results = engine.search(&query, 2, RankerKind::Bm25).unwrap();
    assert_eq!(results.hits.len(), 2);
    assert_eq!(results.total_matches, 5);
    assert!(results.hits[0].score >= results.hits[1].score);
}

#[test]
fn stop_word_only_query_is_rejected() {
    let (_dir, engine) = build(rust_corpus(), Codec::VByte);
    assert!(engine.analyze("the AND of", QueryMode::Boolean).is_err());
    assert!(engine.analyze("rust AND (", QueryMode::Boolean).is_err());
}

#[test]
fn repeated_queries_hit_the_hot_term_cache() {
    let (_dir, engine) = build(rust_corpus(), Codec::VByte);
    let query = engine.analyze("rust memory", QueryMode::Boolean).unwrap();
    engine.search(&query, 10, RankerKind::Bm25).unwrap();
    let cold = engine.hot_term_stats();
    assert_eq!((cold.hits, cold.misses, cold.resident_terms), (0, 2, 2));

    engine.search(&query, 10, RankerKind::Bm25).unwrap();
    let warm = engine.hot_term_stats();
    assert_eq!((warm.hits, warm.misses), (2, 2));
}

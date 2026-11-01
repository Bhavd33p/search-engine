//! The linear-scan baseline and the inverted index must return identical
//! rankings; each checks the other.

use search_engine::analysis::Analyzer;
use search_engine::baseline::LinearScan;
use search_engine::corpus::Document;
use search_engine::engine::{Engine, EngineConfig, QueryMode};
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store::write_index;
use search_engine::rank::{Bm25, RankerKind};

fn corpus() -> Vec<Document> {
    let pages = [
        (
            "Ownership",
            "Each value in Rust has an owner. There can only be one owner at a time. When the owner goes out of scope, the value will be dropped.",
        ),
        (
            "References and Borrowing",
            "A reference is like a pointer: an address we can follow to access data owned by some other variable. Borrowing does not take ownership.",
        ),
        (
            "The Slice Type",
            "Slices let you reference a contiguous sequence of elements in a collection rather than the whole collection. A slice is a kind of reference, so it does not have ownership.",
        ),
        (
            "Unsafe Rust",
            "Unsafe Rust exists because static analysis is conservative. Unsafe superpowers include dereferencing a raw pointer and calling an unsafe function.",
        ),
        (
            "Smart Pointers",
            "A pointer is a general concept for a variable that contains an address in memory. Smart pointers such as Box and Rc have additional metadata and capabilities, like reference counting.",
        ),
        (
            "Fearless Concurrency",
            "Handling concurrent programming safely and efficiently is another of Rust's major goals. Ownership and type checking help manage memory safety and concurrency problems.",
        ),
        (
            "Lifetimes",
            "Every reference in Rust has a lifetime, which is the scope for which that reference is valid. The borrow checker compares scopes to determine whether all borrows are valid.",
        ),
        (
            "Garbage Collection",
            "Some languages have garbage collection that regularly looks for no-longer-used memory; Rust uses ownership with a set of rules that the compiler checks.",
        ),
    ];
    pages
        .iter()
        .enumerate()
        .map(|(i, (title, body))| Document {
            id: i.to_string(),
            url: String::new(),
            title: title.to_string(),
            body: body.to_string(),
        })
        .collect()
}

#[test]
fn baseline_and_index_return_identical_rankings() {
    let documents = corpus();
    let analyzer = Analyzer::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index");
    write_index(&path, &InMemoryIndex::build(&analyzer, documents.clone()), Codec::VByte).unwrap();
    let engine = Engine::open(&path, EngineConfig::default()).unwrap();
    let baseline = LinearScan::new(&analyzer, &documents, Bm25::default());

    let queries = [
        ("ownership", QueryMode::Boolean),
        ("reference pointer", QueryMode::Boolean),
        ("reference OR pointer", QueryMode::Boolean),
        ("rust -unsafe", QueryMode::Boolean),
        ("\"memory safety\"", QueryMode::Boolean),
        ("\"raw pointer\" OR (borrow checker)", QueryMode::Boolean),
        ("(scope OR scopes) -lifetime", QueryMode::Boolean),
        ("-ownership", QueryMode::Boolean),
        (
            "how does rust manage memory without a garbage collector",
            QueryMode::AnyTerms,
        ),
        ("the borrow checker compares scopes", QueryMode::AnyTerms),
    ];
    for (text, mode) in queries {
        let query = engine.analyze(text, mode).unwrap();
        let indexed = engine.rank(&query, 10, RankerKind::Bm25).unwrap().docs;
        let scanned = baseline.search(&query, 10);
        assert_eq!(indexed.len(), scanned.len(), "{text}");
        for (a, b) in indexed.iter().zip(&scanned) {
            assert_eq!(a.doc_id, b.doc_id, "{text}");
            assert!((a.score - b.score).abs() < 1e-9, "{text}: {} vs {}", a.score, b.score);
        }
    }
}

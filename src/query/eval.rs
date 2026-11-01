//! Evaluates an analysed query to the sorted list of documents that match it.

use std::convert::Infallible;
use std::sync::Arc;

use super::analyzed::AnalyzedQuery;
use super::ops::{PhrasePart, difference, intersect, phrase_docs, union};
use crate::index::builder::InMemoryIndex;
use crate::index::postings::PostingList;

/// Anything that can hand out decoded posting lists: the on-disk index (through
/// the hot-term cache) or an in-memory index in tests.
pub trait PostingSource {
    type Error;
    fn postings(&self, term: &str) -> Result<Option<Arc<PostingList>>, Self::Error>;
}

impl PostingSource for InMemoryIndex {
    type Error = Infallible;

    fn postings(&self, term: &str) -> Result<Option<Arc<PostingList>>, Infallible> {
        Ok(InMemoryIndex::postings(self, term).cloned().map(Arc::new))
    }
}

/// Doc ids matching `query`, ascending. `num_docs` defines the universe that a
/// pure negation such as `-unsafe` is taken against.
pub fn matching_docs<S: PostingSource>(query: &AnalyzedQuery, source: &S, num_docs: u32) -> Result<Vec<u32>, S::Error> {
    match query {
        AnalyzedQuery::Term(term) => Ok(source.postings(term)?.map(|list| list.doc_ids()).unwrap_or_default()),
        AnalyzedQuery::Phrase(terms) => {
            let mut lists = Vec::with_capacity(terms.len());
            for part in terms {
                match source.postings(&part.term)? {
                    Some(list) => lists.push((list, part.offset)),
                    None => return Ok(Vec::new()), // a missing word means no phrase match
                }
            }
            let parts: Vec<PhrasePart<'_>> = lists
                .iter()
                .map(|(list, offset)| PhrasePart {
                    postings: list,
                    offset: *offset,
                })
                .collect();
            Ok(phrase_docs(&parts))
        }
        AnalyzedQuery::Or(children) => {
            let mut result = Vec::new();
            for child in children {
                result = union(&result, &matching_docs(child, source, num_docs)?);
            }
            Ok(result)
        }
        AnalyzedQuery::And(children) => {
            // a AND b AND NOT c  =  (a ∩ b) \ c, without materialising "everything except c".
            let mut positives = Vec::new();
            let mut negatives = Vec::new();
            for child in children {
                match child {
                    AnalyzedQuery::Not(inner) => negatives.push(matching_docs(inner, source, num_docs)?),
                    other => positives.push(matching_docs(other, source, num_docs)?),
                }
            }
            // Intersecting the shortest lists first keeps intermediate results small.
            positives.sort_by_key(Vec::len);
            let mut result = match positives.split_first() {
                Some((first, rest)) => rest.iter().fold(first.clone(), |acc, list| intersect(&acc, list)),
                None => all_docs(num_docs),
            };
            for excluded in &negatives {
                result = difference(&result, excluded);
            }
            Ok(result)
        }
        AnalyzedQuery::Not(inner) => Ok(difference(
            &all_docs(num_docs),
            &matching_docs(inner, source, num_docs)?,
        )),
    }
}

fn all_docs(num_docs: u32) -> Vec<u32> {
    (0..num_docs).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analyzer;
    use crate::corpus::Document;
    use crate::query::parse;

    fn index() -> InMemoryIndex {
        let bodies = [
            "Rust guarantees memory safety without garbage collection", // 0
            "Unsafe Rust lets you dereference raw pointers",            // 1
            "Garbage collection pauses hurt latency",                   // 2
            "Ownership and borrowing give Rust memory safety",          // 3
            "Safety of memory is a property, not a feature of the memory", // 4
        ];
        let docs = bodies.iter().enumerate().map(|(i, body)| Document {
            id: i.to_string(),
            url: String::new(),
            title: String::new(),
            body: body.to_string(),
        });
        InMemoryIndex::build(&Analyzer::new(), docs)
    }

    fn search(input: &str) -> Vec<u32> {
        let index = index();
        let query = AnalyzedQuery::analyze(&parse(input).unwrap(), &Analyzer::new()).unwrap();
        let Ok(docs) = matching_docs(&query, &index, index.num_docs());
        docs
    }

    #[test]
    fn boolean_operators() {
        assert_eq!(search("rust"), [0, 1, 3]);
        assert_eq!(search("rust memory"), [0, 3]);
        assert_eq!(search("rust AND memory"), [0, 3]);
        assert_eq!(search("garbage OR unsafe"), [0, 1, 2]);
        assert_eq!(search("rust -unsafe"), [0, 3]);
        assert_eq!(search("rust NOT unsafe"), [0, 3]);
        assert_eq!(search("(garbage OR ownership) rust"), [0, 3]);
    }

    #[test]
    fn pure_negation_is_taken_against_all_documents() {
        assert_eq!(search("-rust"), [2, 4]);
        assert_eq!(search("NOT (rust OR garbage)"), [4]);
    }

    #[test]
    fn phrases_need_adjacent_words_in_order() {
        assert_eq!(search("\"memory safety\""), [0, 3]);
        assert_eq!(search("\"safety memory\""), Vec::<u32>::new());
        assert_eq!(search("\"memory safety\" -ownership"), [0]);
        assert_eq!(search("\"memory safety\" OR latency"), [0, 2, 3]);
    }

    #[test]
    fn phrase_with_unknown_word_matches_nothing() {
        assert!(search("\"memory zebra\"").is_empty());
    }

    #[test]
    fn phrase_respects_stop_word_gaps() {
        // doc 4: "safety of memory" — 'of' is a stop word, so safety@0 memory@2.
        assert_eq!(search("\"safety of memory\""), [4]);
        assert!(search("\"safety memory\"").is_empty());
    }
}

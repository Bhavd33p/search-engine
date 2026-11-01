//! Builds the inverted index in memory.
//!
//! Documents get dense ids `0, 1, 2, …` in the order they are added, so every
//! posting list is born sorted by doc id — no sort needed before encoding.
//!
//! Everything lives in RAM until [`super::store::write_index`] persists it.
//! That is fine for corpora of a few hundred thousand documents; beyond that a
//! real engine flushes sorted *segments* to disk and merges them (future work).

use std::collections::HashMap;

use super::postings::{Posting, PostingList};
use crate::analysis::Analyzer;
use crate::corpus::Document;

pub struct IndexBuilder<'a> {
    analyzer: &'a Analyzer,
    terms: HashMap<String, Vec<Posting>>,
    documents: Vec<Document>,
    doc_lengths: Vec<u32>,
}

/// The finished index, ready to be written to storage.
pub struct InMemoryIndex {
    /// Posting lists sorted by term.
    pub terms: Vec<(String, PostingList)>,
    /// Stored fields, indexed by doc id.
    pub documents: Vec<Document>,
    /// Number of indexed terms per document (after stop-word removal).
    pub doc_lengths: Vec<u32>,
}

impl<'a> IndexBuilder<'a> {
    pub fn new(analyzer: &'a Analyzer) -> Self {
        Self {
            analyzer,
            terms: HashMap::new(),
            documents: Vec::new(),
            doc_lengths: Vec::new(),
        }
    }

    /// Analyses and inverts one document; returns its doc id.
    pub fn add(&mut self, document: Document) -> u32 {
        let doc_id = self.documents.len() as u32;
        let tokens = self.analyzer.analyze_document(&document.title, &document.body);

        // Tokens arrive in position order, so each term's positions are already sorted.
        let mut positions_by_term: HashMap<String, Vec<u32>> = HashMap::new();
        for token in &tokens {
            positions_by_term
                .entry(token.term.clone())
                .or_default()
                .push(token.position);
        }
        for (term, positions) in positions_by_term {
            self.terms.entry(term).or_default().push(Posting { doc_id, positions });
        }

        self.doc_lengths.push(tokens.len() as u32);
        self.documents.push(document);
        doc_id
    }

    pub fn finish(self) -> InMemoryIndex {
        let mut terms: Vec<(String, PostingList)> = self
            .terms
            .into_iter()
            .map(|(term, postings)| (term, PostingList::new(postings)))
            .collect();
        terms.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        InMemoryIndex {
            terms,
            documents: self.documents,
            doc_lengths: self.doc_lengths,
        }
    }
}

impl InMemoryIndex {
    pub fn build(analyzer: &Analyzer, documents: impl IntoIterator<Item = Document>) -> Self {
        let mut builder = IndexBuilder::new(analyzer);
        for document in documents {
            builder.add(document);
        }
        builder.finish()
    }

    pub fn num_docs(&self) -> u32 {
        self.documents.len() as u32
    }

    pub fn postings(&self, term: &str) -> Option<&PostingList> {
        self.terms
            .binary_search_by(|(t, _)| t.as_str().cmp(term))
            .ok()
            .map(|i| &self.terms[i].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::FIELD_POSITION_GAP;
    use crate::index::postings::{Codec, encode};

    fn doc(id: &str, title: &str, body: &str) -> Document {
        Document {
            id: id.into(),
            url: String::new(),
            title: title.into(),
            body: body.into(),
        }
    }

    #[test]
    fn inverts_documents_into_positional_postings() {
        let analyzer = Analyzer::new();
        let index = InMemoryIndex::build(
            &analyzer,
            [
                doc("a", "", "The quick brown foxes"),
                doc("b", "", "a fox jumps over the lazy fox"),
            ],
        );

        assert_eq!(index.num_docs(), 2);
        assert_eq!(index.doc_lengths, [3, 4]);

        // Bodies start after the (empty) title plus the field gap.
        let fox = index.postings("fox").unwrap();
        assert_eq!(fox.doc_ids(), [0, 1]);
        assert_eq!(fox.get(0).unwrap().positions, [FIELD_POSITION_GAP + 3]);
        assert_eq!(
            fox.get(1).unwrap().positions,
            [FIELD_POSITION_GAP + 1, FIELD_POSITION_GAP + 6]
        );

        assert!(index.postings("the").is_none(), "stop words are not indexed");
        let terms: Vec<&str> = index.terms.iter().map(|(t, _)| t.as_str()).collect();
        assert!(terms.windows(2).all(|w| w[0] < w[1]), "terms are sorted");
    }

    #[test]
    fn walkthrough_sentence_becomes_the_documented_bytes() {
        // docs/WALKTHROUGH.md: "The quick brown foxes" as the title of doc 0.
        let analyzer = Analyzer::new();
        let index = InMemoryIndex::build(&analyzer, [doc("0", "The quick brown foxes", "")]);
        let terms: Vec<&str> = index.terms.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(terms, ["brown", "fox", "quick"]);
        let fox = index.postings("fox").unwrap();
        assert_eq!(fox.get(0).unwrap().positions, [3]);
        assert_eq!(encode(fox, Codec::VByte), [0x01, 0x00, 0x01, 0x03]);
        assert_eq!(
            encode(fox, Codec::FixedU32),
            [1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0]
        );
    }

    #[test]
    fn title_and_body_are_both_indexed() {
        let analyzer = Analyzer::new();
        let index = InMemoryIndex::build(&analyzer, [doc("a", "Ownership", "borrowing")]);
        assert!(index.postings("ownership").is_some());
        assert!(index.postings("borrow").is_some());
    }
}

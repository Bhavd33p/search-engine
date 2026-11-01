//! Text analysis: turns raw text into the terms that go into the index.
//!
//! The same [`Analyzer`] runs over documents at index time and over queries at
//! search time, so "Foxes" in a query meets "fox" in the index.
//!
//! Steps for every word:
//! 1. split on Unicode word boundaries (UAX #29), so "naïve", "don't" and "3.14" stay whole;
//! 2. lowercase;
//! 3. drop stop words (but still count their position);
//! 4. stem with the Snowball English (Porter2) stemmer — the one piece taken from a crate.

mod stopwords;

use std::collections::HashSet;

use rust_stemmers::{Algorithm, Stemmer};
use unicode_segmentation::UnicodeSegmentation;

/// Words longer than this are almost always junk (base64, hashes, minified code).
const MAX_WORD_BYTES: usize = 64;

/// Positions skipped between a document's title and body so that a phrase can
/// never match across the boundary.
pub const FIELD_POSITION_GAP: u32 = 100;

/// One indexed term with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The normalised, stemmed term, e.g. `"fox"` for `"Foxes"`.
    pub term: String,
    /// Word position in the text, counting stop words too.
    pub position: u32,
    /// Byte range of the original word in the input text.
    pub start: usize,
    pub end: usize,
}

pub struct Analyzer {
    stemmer: Stemmer,
    stop_words: HashSet<&'static str>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            stemmer: Stemmer::create(Algorithm::English),
            stop_words: stopwords::ENGLISH.iter().copied().collect(),
        }
    }

    /// Analyses `text`, numbering positions from 0.
    pub fn analyze(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        self.analyze_into(text, 0, &mut tokens);
        tokens
    }

    /// Analyses a document's title and body as one token stream, with a
    /// position gap between the two fields.
    pub fn analyze_document(&self, title: &str, body: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let next_position = self.analyze_into(title, 0, &mut tokens);
        self.analyze_into(body, next_position + FIELD_POSITION_GAP, &mut tokens);
        tokens
    }

    /// Position of a document's first body word: the title's words come
    /// first, then [`FIELD_POSITION_GAP`]. Mirrors [`Self::analyze_document`].
    pub fn body_start_position(&self, title: &str) -> u32 {
        title.unicode_words().count() as u32 + FIELD_POSITION_GAP
    }

    /// Normalises a single word to its index term, or `None` for stop words and junk.
    pub fn normalize_word(&self, word: &str) -> Option<String> {
        if word.len() > MAX_WORD_BYTES {
            return None;
        }
        // Curly apostrophes are common in web text; fold them so "don’t" is a stop word too.
        let lowered = word.to_lowercase().replace('\u{2019}', "'");
        if self.stop_words.contains(lowered.as_str()) {
            return None;
        }
        Some(self.stemmer.stem(&lowered).into_owned())
    }

    /// Appends tokens for `text` starting at `first_position`; returns the next free position.
    fn analyze_into(&self, text: &str, first_position: u32, tokens: &mut Vec<Token>) -> u32 {
        let mut position = first_position;
        for (start, word) in text.unicode_word_indices() {
            if let Some(term) = self.normalize_word(word) {
                tokens.push(Token {
                    term,
                    position,
                    start,
                    end: start + word.len(),
                });
            }
            position += 1;
        }
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(text: &str) -> Vec<String> {
        Analyzer::new().analyze(text).into_iter().map(|t| t.term).collect()
    }

    #[test]
    fn lowercases_and_stems() {
        assert_eq!(terms("The quick brown foxes"), ["quick", "brown", "fox"]);
        assert_eq!(
            terms("connection connected connecting"),
            ["connect", "connect", "connect"]
        );
        assert_eq!(terms("Running RUNS"), ["run", "run"]);
    }

    #[test]
    fn stop_words_are_dropped_but_keep_their_positions() {
        let tokens = Analyzer::new().analyze("The quick brown foxes");
        let positions: Vec<u32> = tokens.iter().map(|t| t.position).collect();
        assert_eq!(positions, [1, 2, 3]);
    }

    #[test]
    fn curly_apostrophes_are_folded_before_the_stop_word_check() {
        assert!(terms("don’t won't").is_empty());
    }

    #[test]
    fn splits_on_unicode_word_boundaries() {
        assert_eq!(terms("naïve café, 3.14 π"), ["naïv", "café", "3.14", "π"]);
        assert_eq!(terms("HashMap::new()"), ["hashmap", "new"]);
    }

    #[test]
    fn punctuation_only_text_has_no_tokens() {
        assert!(terms(" -- ... !!! ").is_empty());
        assert!(terms("").is_empty());
    }

    #[test]
    fn byte_offsets_point_at_the_original_word() {
        let text = "Ownership in Rust";
        let tokens = Analyzer::new().analyze(text);
        assert_eq!(&text[tokens[0].start..tokens[0].end], "Ownership");
        assert_eq!(&text[tokens[1].start..tokens[1].end], "Rust");
    }

    #[test]
    fn very_long_words_are_skipped() {
        let junk = "a".repeat(MAX_WORD_BYTES + 1);
        assert_eq!(terms(&format!("{junk} borrow")), ["borrow"]);
    }

    #[test]
    fn document_fields_are_separated_by_a_position_gap() {
        let analyzer = Analyzer::new();
        let tokens = analyzer.analyze_document("Ownership", "borrowing rules");
        let positions: Vec<u32> = tokens.iter().map(|t| t.position).collect();
        assert_eq!(positions, [0, 1 + FIELD_POSITION_GAP, 2 + FIELD_POSITION_GAP]);
        assert_eq!(analyzer.body_start_position("Ownership"), 1 + FIELD_POSITION_GAP);
        assert_eq!(analyzer.body_start_position("The Rust Book"), 3 + FIELD_POSITION_GAP);
    }
}

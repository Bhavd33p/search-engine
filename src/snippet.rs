//! Builds a short text excerpt around a query match, with matches in **bold**.
//!
//! The engine already knows *where* the match is: the positional postings give
//! the word position of each query term in the document. So the snippet only
//! has to segment the text up to that word and stem the ~30 words it shows,
//! instead of stemming the whole document looking for a match.

use std::collections::BTreeMap;

use unicode_segmentation::UnicodeSegmentation;

use crate::analysis::Analyzer;

/// Up to `max_words` words of `text`, starting a little before word number
/// `anchor` (or at the beginning when there is no anchor). Words whose
/// analysed term is in `terms` are highlighted.
pub fn make_snippet(
    analyzer: &Analyzer,
    text: &str,
    terms: &BTreeMap<String, u32>,
    anchor: Option<usize>,
    max_words: usize,
) -> String {
    let start = anchor.map_or(0, |word| word.saturating_sub(max_words / 4));
    // One word past the window tells us whether to add a trailing ellipsis.
    let window: Vec<(usize, &str)> = text.unicode_word_indices().skip(start).take(max_words + 1).collect();
    let shown = window.len().min(max_words);
    if shown == 0 {
        return String::new();
    }

    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str("… ");
    }
    let mut previous_end = window[0].0;
    for &(offset, word) in &window[..shown] {
        // Keep the punctuation between words, but collapse runs of whitespace.
        push_collapsing_whitespace(&text[previous_end..offset], &mut snippet);
        if analyzer
            .normalize_word(word)
            .is_some_and(|term| terms.contains_key(&term))
        {
            snippet.push_str("**");
            snippet.push_str(word);
            snippet.push_str("**");
        } else {
            snippet.push_str(word);
        }
        previous_end = offset + word.len();
    }
    if window.len() > shown {
        snippet.push_str(" …");
    }
    snippet
}

fn push_collapsing_whitespace(text: &str, out: &mut String) {
    let mut in_whitespace = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_whitespace {
                out.push(' ');
            }
            in_whitespace = true;
        } else {
            out.push(c);
            in_whitespace = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(words: &[&str]) -> BTreeMap<String, u32> {
        words.iter().map(|w| (w.to_string(), 1)).collect()
    }

    #[test]
    fn highlights_matches_and_keeps_punctuation() {
        let snippet = make_snippet(
            &Analyzer::new(),
            "Rust is fast, and memory-safe.",
            &terms(&["memori"]),
            Some(4),
            20,
        );
        assert_eq!(snippet, "Rust is fast, and **memory**-safe");
    }

    #[test]
    fn windows_around_the_anchor_word() {
        let text = (0..100).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ") + " target and more words";
        let snippet = make_snippet(&Analyzer::new(), &text, &terms(&["target"]), Some(100), 8);
        assert_eq!(snippet, "… w98 w99 **target** and more words");
    }

    #[test]
    fn starts_at_the_beginning_without_an_anchor() {
        let snippet = make_snippet(&Analyzer::new(), "one two three four", &terms(&["zzz"]), None, 2);
        assert_eq!(snippet, "one two …");
    }

    #[test]
    fn anchor_past_the_end_gives_empty_snippet() {
        assert_eq!(
            make_snippet(&Analyzer::new(), "one two", &terms(&["one"]), Some(50), 4),
            ""
        );
        assert_eq!(make_snippet(&Analyzer::new(), "", &terms(&["a"]), None, 10), "");
    }
}

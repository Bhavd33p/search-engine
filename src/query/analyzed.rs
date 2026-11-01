//! Runs the text analyser over a parsed query, so query words become the
//! same stemmed terms that are in the index.

use std::collections::BTreeMap;
use std::fmt;

use super::Query;
use crate::analysis::Analyzer;

/// One word of a phrase after analysis, with its offset from the phrase's first word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhraseTerm {
    pub term: String,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzedQuery {
    Term(String),
    /// Two or more terms that must appear at these relative positions.
    Phrase(Vec<PhraseTerm>),
    And(Vec<AnalyzedQuery>),
    Or(Vec<AnalyzedQuery>),
    Not(Box<AnalyzedQuery>),
}

impl AnalyzedQuery {
    /// Analyses every word of `query`. Returns `None` when nothing searchable is
    /// left (e.g. the query was only stop words).
    ///
    /// * a word that is a stop word disappears from its AND/OR;
    /// * a word that splits into several terms ("state-of-the-art") becomes a phrase;
    /// * a phrase that analyses to a single term becomes a plain term.
    pub fn analyze(query: &Query, analyzer: &Analyzer) -> Option<AnalyzedQuery> {
        match query {
            Query::Term(text) | Query::Phrase(text) => Self::from_text(text, analyzer),
            Query::And(children) => Self::combine(
                children.iter().filter_map(|c| Self::analyze(c, analyzer)).collect(),
                Self::And,
            ),
            Query::Or(children) => Self::combine(
                children.iter().filter_map(|c| Self::analyze(c, analyzer)).collect(),
                Self::Or,
            ),
            Query::Not(inner) => Self::analyze(inner, analyzer).map(|q| Self::Not(Box::new(q))),
        }
    }

    /// Free-text ("any word") query: an OR of every term in `text`. Used to
    /// evaluate natural-language queries such as Cranfield's.
    pub fn any_terms(text: &str, analyzer: &Analyzer) -> Option<AnalyzedQuery> {
        let terms = analyzer
            .analyze(text)
            .into_iter()
            .map(|token| Self::Term(token.term))
            .collect();
        Self::combine(terms, Self::Or)
    }

    fn from_text(text: &str, analyzer: &Analyzer) -> Option<AnalyzedQuery> {
        let tokens = analyzer.analyze(text);
        match tokens.as_slice() {
            [] => None,
            [single] => Some(Self::Term(single.term.clone())),
            [first, ..] => Some(Self::Phrase(
                tokens
                    .iter()
                    .map(|t| PhraseTerm {
                        term: t.term.clone(),
                        offset: t.position - first.position,
                    })
                    .collect(),
            )),
        }
    }

    fn combine(mut children: Vec<AnalyzedQuery>, wrap: fn(Vec<AnalyzedQuery>) -> AnalyzedQuery) -> Option<Self> {
        match children.len() {
            0 => None,
            1 => children.pop(),
            _ => Some(wrap(children)),
        }
    }

    /// Terms that contribute to the score, with how often each appears in the
    /// query. Terms under a NOT only filter, so they are left out.
    pub fn scoring_terms(&self) -> BTreeMap<String, u32> {
        let mut terms = BTreeMap::new();
        self.collect_scoring_terms(false, &mut terms);
        terms
    }

    fn collect_scoring_terms(&self, negated: bool, terms: &mut BTreeMap<String, u32>) {
        match self {
            Self::Term(term) if !negated => *terms.entry(term.clone()).or_default() += 1,
            Self::Phrase(parts) if !negated => {
                for part in parts {
                    *terms.entry(part.term.clone()).or_default() += 1;
                }
            }
            Self::Term(_) | Self::Phrase(_) => {}
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_scoring_terms(negated, terms);
                }
            }
            Self::Not(inner) => inner.collect_scoring_terms(!negated, terms),
        }
    }

    /// A normalised string that is equal for queries that must return equal
    /// results. AND/OR children are sorted because their order never matters.
    /// Used as part of the result-cache key.
    pub fn canonical(&self) -> String {
        match self {
            Self::Term(term) => escape(term),
            Self::Phrase(parts) => {
                let words: Vec<String> = parts
                    .iter()
                    .map(|p| format!("{}@{}", escape(&p.term), p.offset))
                    .collect();
                format!("\"{}\"", words.join(" "))
            }
            Self::And(children) => Self::canonical_group("and", children),
            Self::Or(children) => Self::canonical_group("or", children),
            Self::Not(inner) => format!("not({})", inner.canonical()),
        }
    }

    fn canonical_group(name: &str, children: &[AnalyzedQuery]) -> String {
        let mut parts: Vec<String> = children.iter().map(Self::canonical).collect();
        parts.sort_unstable();
        format!("{name}({})", parts.join(","))
    }
}

/// Terms can contain commas ("1,000" is one word). Unescaped, the queries
/// `1,2 x` and `1 2 x` would both become `and(1,2,x)` and share a cache entry.
fn escape(term: &str) -> String {
    if term.contains([',', '\\']) {
        term.replace('\\', "\\\\").replace(',', "\\,")
    } else {
        term.to_string()
    }
}

impl fmt::Display for AnalyzedQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse;

    fn analyze(input: &str) -> Option<AnalyzedQuery> {
        AnalyzedQuery::analyze(&parse(input).unwrap(), &Analyzer::new())
    }

    #[test]
    fn stems_terms_and_drops_stop_words() {
        assert_eq!(
            analyze("the running foxes").unwrap(),
            AnalyzedQuery::And(vec![
                AnalyzedQuery::Term("run".into()),
                AnalyzedQuery::Term("fox".into())
            ])
        );
        assert_eq!(analyze("the AND a"), None);
        assert_eq!(analyze("rust -the").unwrap(), AnalyzedQuery::Term("rust".into()));
    }

    #[test]
    fn phrases_keep_stop_word_gaps_as_offsets() {
        let phrase = analyze("\"wing in a slipstream\"").unwrap();
        assert_eq!(phrase.canonical(), "\"wing@0 slipstream@3\"");
    }

    #[test]
    fn hyphenated_word_becomes_a_phrase() {
        assert_eq!(analyze("state-of-the-art").unwrap().canonical(), "\"state@0 art@3\"");
    }

    #[test]
    fn single_word_phrase_is_a_term() {
        assert_eq!(
            analyze("\"ownership\"").unwrap(),
            AnalyzedQuery::Term("ownership".into())
        );
    }

    #[test]
    fn scoring_terms_skip_negated_terms_and_count_repeats() {
        let query = analyze("rust (borrow OR rust) -unsafe \"borrow checker\"").unwrap();
        let terms: Vec<(String, u32)> = query.scoring_terms().into_iter().collect();
        assert_eq!(terms, [("borrow".into(), 2), ("checker".into(), 1), ("rust".into(), 2)]);
    }

    #[test]
    fn canonical_form_ignores_operand_order_and_spelling() {
        assert_eq!(
            analyze("Foxes AND quick").unwrap().canonical(),
            analyze("quick fox").unwrap().canonical()
        );
        assert_ne!(
            analyze("quick fox").unwrap().canonical(),
            analyze("quick OR fox").unwrap().canonical()
        );
    }

    #[test]
    fn commas_inside_terms_cannot_collide_with_separators() {
        let one_term = analyze("1,000 x").unwrap().canonical();
        let two_terms = analyze("1 000 x").unwrap().canonical();
        assert_eq!(one_term, "and(1\\,000,x)");
        assert_eq!(two_terms, "and(000,1,x)");
        let mixed = analyze("1,2 x").unwrap().canonical();
        let separate = analyze("1 2 x").unwrap().canonical();
        assert_ne!(mixed, separate);
    }

    #[test]
    fn any_terms_ors_every_word() {
        let query = AnalyzedQuery::any_terms("what is the borrow checker", &Analyzer::new()).unwrap();
        assert_eq!(query.canonical(), "or(borrow,checker)");
    }
}

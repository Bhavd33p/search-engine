//! The query language: parsing, analysis and boolean/phrase evaluation.
//!
//! ```text
//! rust ownership                 both terms (implicit AND)
//! rust AND ownership             the same, explicitly
//! borrow OR lifetime             either term
//! rust NOT unsafe   rust -unsafe documents with "rust" but not "unsafe"
//! "memory safety"                the exact phrase
//! (ownership OR borrow) -unsafe  grouping
//! ```

mod lexer;
mod parser;

use std::fmt;

use thiserror::Error;

pub use parser::parse;

/// A parsed query, before text analysis. Words are kept exactly as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Term(String),
    Phrase(String),
    And(Vec<Query>),
    Or(Vec<Query>),
    Not(Box<Query>),
}

impl Query {
    /// Builds an AND node, flattening nested ANDs and unwrapping a single operand.
    pub fn and(operands: Vec<Query>) -> Query {
        Self::flatten(operands, Query::And, |q| match q {
            Query::And(children) => Ok(children),
            other => Err(other),
        })
    }

    /// Builds an OR node, flattening nested ORs and unwrapping a single operand.
    pub fn or(operands: Vec<Query>) -> Query {
        Self::flatten(operands, Query::Or, |q| match q {
            Query::Or(children) => Ok(children),
            other => Err(other),
        })
    }

    fn flatten(
        operands: Vec<Query>,
        wrap: fn(Vec<Query>) -> Query,
        unwrap_same: fn(Query) -> Result<Vec<Query>, Query>,
    ) -> Query {
        let mut flat = Vec::with_capacity(operands.len());
        for operand in operands {
            match unwrap_same(operand) {
                Ok(children) => flat.extend(children),
                Err(other) => flat.push(other),
            }
        }
        if flat.len() == 1 { flat.remove(0) } else { wrap(flat) }
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let join = |f: &mut fmt::Formatter<'_>, name: &str, children: &[Query]| {
            write!(f, "{name}(")?;
            for (i, child) in children.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{child}")?;
            }
            f.write_str(")")
        };
        match self {
            Query::Term(word) => f.write_str(word),
            Query::Phrase(text) => write!(f, "\"{text}\""),
            Query::And(children) => join(f, "AND", children),
            Query::Or(children) => join(f, "OR", children),
            Query::Not(inner) => write!(f, "NOT({inner})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("empty query")]
    Empty,
    #[error("unterminated phrase starting at character {offset}")]
    UnterminatedPhrase { offset: usize },
    #[error("missing ')'")]
    UnclosedParen,
    #[error("unmatched ')'")]
    UnmatchedRightParen,
    #[error("query ends where a term was expected")]
    UnexpectedEnd,
    #[error("unexpected {0}")]
    Unexpected(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_shows_the_tree() {
        let query = parse("rust (ownership OR borrow) -\"unsafe code\"").unwrap();
        assert_eq!(
            query.to_string(),
            "AND(rust, OR(ownership, borrow), NOT(\"unsafe code\"))"
        );
    }
}

//! Recursive-descent parser for the query language.
//!
//! Grammar, from lowest to highest precedence (each rule is one function):
//!
//! ```text
//! or_expr  := and_expr ( "OR" and_expr )*
//! and_expr := unary ( "AND"? unary )*          juxtaposition means AND
//! unary    := ( "NOT" | "-" ) unary | primary
//! primary  := WORD | PHRASE | "(" or_expr ")"
//! ```
//!
//! So `a OR b c` is `a OR (b AND c)`, and `NOT a b` is `(NOT a) AND b`.

use super::lexer::{Token, tokenize};
use super::{ParseError, Query};

pub fn parse(input: &str) -> Result<Query, ParseError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut parser = Parser { tokens, position: 0 };
    let query = parser.or_expr()?;
    match parser.peek() {
        None => Ok(query),
        Some(Token::RightParen) => Err(ParseError::UnmatchedRightParen),
        Some(token) => Err(ParseError::Unexpected(token.describe())),
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        token
    }

    fn or_expr(&mut self) -> Result<Query, ParseError> {
        let mut operands = vec![self.and_expr()?];
        while self.peek() == Some(&Token::Or) {
            self.next();
            operands.push(self.and_expr()?);
        }
        Ok(Query::or(operands))
    }

    fn and_expr(&mut self) -> Result<Query, ParseError> {
        let mut operands = vec![self.unary()?];
        loop {
            match self.peek() {
                Some(Token::And) => {
                    self.next();
                    operands.push(self.unary()?);
                }
                // Implicit AND: anything that can start an operand.
                Some(Token::Word(_) | Token::Phrase(_) | Token::LeftParen | Token::Not | Token::Minus) => {
                    operands.push(self.unary()?);
                }
                _ => break,
            }
        }
        Ok(Query::and(operands))
    }

    fn unary(&mut self) -> Result<Query, ParseError> {
        match self.peek() {
            Some(Token::Not | Token::Minus) => {
                self.next();
                Ok(Query::Not(Box::new(self.unary()?)))
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<Query, ParseError> {
        match self.next() {
            Some(Token::Word(word)) => Ok(Query::Term(word)),
            Some(Token::Phrase(text)) => Ok(Query::Phrase(text)),
            Some(Token::LeftParen) => {
                let inner = self.or_expr()?;
                match self.next() {
                    Some(Token::RightParen) => Ok(inner),
                    _ => Err(ParseError::UnclosedParen),
                }
            }
            Some(token) => Err(ParseError::Unexpected(token.describe())),
            None => Err(ParseError::UnexpectedEnd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(w: &str) -> Query {
        Query::Term(w.into())
    }

    fn not(q: Query) -> Query {
        Query::Not(Box::new(q))
    }

    #[test]
    fn juxtaposition_is_and() {
        assert_eq!(
            parse("rust ownership").unwrap(),
            Query::And(vec![term("rust"), term("ownership")])
        );
        assert_eq!(parse("rust AND ownership").unwrap(), parse("rust ownership").unwrap());
    }

    #[test]
    fn and_binds_tighter_than_or() {
        assert_eq!(
            parse("a OR b c").unwrap(),
            Query::Or(vec![term("a"), Query::And(vec![term("b"), term("c")])])
        );
        assert_eq!(
            parse("a b OR c").unwrap(),
            Query::Or(vec![Query::And(vec![term("a"), term("b")]), term("c")])
        );
    }

    #[test]
    fn not_binds_tightest() {
        assert_eq!(parse("NOT a b").unwrap(), Query::And(vec![not(term("a")), term("b")]));
        assert_eq!(parse("a -b").unwrap(), Query::And(vec![term("a"), not(term("b"))]));
        assert_eq!(parse("-(a OR b)").unwrap(), not(Query::Or(vec![term("a"), term("b")])));
    }

    #[test]
    fn parentheses_override_precedence() {
        assert_eq!(
            parse("(a OR b) c").unwrap(),
            Query::And(vec![Query::Or(vec![term("a"), term("b")]), term("c")])
        );
    }

    #[test]
    fn same_operator_chains_are_flattened() {
        assert_eq!(
            parse("a OR (b OR c)").unwrap(),
            Query::Or(vec![term("a"), term("b"), term("c")])
        );
        assert_eq!(
            parse("a (b c)").unwrap(),
            Query::And(vec![term("a"), term("b"), term("c")])
        );
    }

    #[test]
    fn phrases_are_operands() {
        assert_eq!(
            parse("\"memory safety\" -unsafe").unwrap(),
            Query::And(vec![Query::Phrase("memory safety".into()), not(term("unsafe"))])
        );
    }

    #[test]
    fn reports_syntax_errors() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("   "), Err(ParseError::Empty));
        assert_eq!(parse("(a OR b"), Err(ParseError::UnclosedParen));
        assert_eq!(parse("a)"), Err(ParseError::UnmatchedRightParen));
        assert_eq!(parse("a OR"), Err(ParseError::UnexpectedEnd));
        assert_eq!(parse("AND a"), Err(ParseError::Unexpected("AND".into())));
        assert_eq!(parse("a OR OR b"), Err(ParseError::Unexpected("OR".into())));
        assert_eq!(parse("NOT"), Err(ParseError::UnexpectedEnd));
    }
}

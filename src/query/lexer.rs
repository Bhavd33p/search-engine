//! Splits a query string into tokens for the parser.

use super::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    /// Text between double quotes.
    Phrase(String),
    And,
    Or,
    Not,
    /// A `-` glued to the front of a word, phrase or group: `-unsafe`.
    Minus,
    LeftParen,
    RightParen,
}

impl Token {
    pub fn describe(&self) -> String {
        match self {
            Token::Word(word) => format!("word '{word}'"),
            Token::Phrase(text) => format!("phrase \"{text}\""),
            Token::And => "AND".into(),
            Token::Or => "OR".into(),
            Token::Not => "NOT".into(),
            Token::Minus => "'-'".into(),
            Token::LeftParen => "'('".into(),
            Token::RightParen => "')'".into(),
        }
    }
}

/// Operators are recognised only in upper case, so "and"/"or"/"not" stay
/// ordinary words. A `-` negates only at the start of a token: "-unsafe"
/// excludes, "state-of-the-art" is one word.
pub fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some(&(offset, c)) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::LeftParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RightParen);
            }
            '"' => {
                chars.next();
                let mut text = String::new();
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    text.push(c);
                }
                if !closed {
                    return Err(ParseError::UnterminatedPhrase { offset });
                }
                tokens.push(Token::Phrase(text));
            }
            '-' => {
                chars.next();
                match chars.peek() {
                    Some(&(_, next)) if !next.is_whitespace() && !matches!(next, ')' | '-') => {
                        tokens.push(Token::Minus)
                    }
                    _ => {} // a '-' that does not start an operand is punctuation, not an operator
                }
            }
            _ => {
                let mut word = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '(' | ')' | '"') {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                tokens.push(match word.as_str() {
                    "AND" => Token::And,
                    "OR" => Token::Or,
                    "NOT" => Token::Not,
                    _ => Token::Word(word),
                });
            }
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str) -> Token {
        Token::Word(w.into())
    }

    #[test]
    fn splits_words_operators_and_parens() {
        let tokens = tokenize("rust AND (ownership OR borrow) NOT gc").unwrap();
        assert_eq!(
            tokens,
            [
                word("rust"),
                Token::And,
                Token::LeftParen,
                word("ownership"),
                Token::Or,
                word("borrow"),
                Token::RightParen,
                Token::Not,
                word("gc"),
            ]
        );
    }

    #[test]
    fn lowercase_operators_are_plain_words() {
        assert_eq!(
            tokenize("cats and dogs").unwrap(),
            [word("cats"), word("and"), word("dogs")]
        );
    }

    #[test]
    fn quotes_make_a_phrase() {
        assert_eq!(
            tokenize("\"memory safety\" rust").unwrap(),
            [Token::Phrase("memory safety".into()), word("rust")]
        );
    }

    #[test]
    fn unterminated_phrase_is_an_error() {
        assert_eq!(
            tokenize("rust \"memory"),
            Err(ParseError::UnterminatedPhrase { offset: 5 })
        );
    }

    #[test]
    fn leading_minus_negates_but_inner_hyphen_does_not() {
        assert_eq!(tokenize("-unsafe").unwrap(), [Token::Minus, word("unsafe")]);
        assert_eq!(
            tokenize("-\"a b\"").unwrap(),
            [Token::Minus, Token::Phrase("a b".into())]
        );
        assert_eq!(tokenize("state-of-the-art").unwrap(), [word("state-of-the-art")]);
        assert_eq!(tokenize("a - b").unwrap(), [word("a"), word("b")]);
        assert_eq!(tokenize("a -- --b").unwrap(), [word("a"), Token::Minus, word("b")]);
        assert_eq!(
            tokenize("(a -)").unwrap(),
            [Token::LeftParen, word("a"), Token::RightParen]
        );
    }
}

use std::ops::Range;

use super::{QueryParseError, QueryParseErrorKind};

#[derive(Debug)]
pub(super) enum TokenKind {
    Open,
    Close,
    And,
    Or,
    Not,
    Filter { key: String, value: String },
}

#[derive(Debug)]
pub(super) struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
    pub separated: bool,
}

fn normalize_quote(c: char) -> char {
    match c {
        '“' | '”' => '"',
        '‘' | '’' => '\'',
        _ => c,
    }
}

struct Lexer<'a> {
    input: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn quoted(&mut self, start: usize) -> Result<String, QueryParseError> {
        let opening = self.peek().expect("quoted string has opening delimiter");
        let delimiter = normalize_quote(opening);
        self.pos += opening.len_utf8();
        let mut value = String::new();
        while let Some(c) = self.peek() {
            let escape_start = self.pos;
            self.pos += c.len_utf8();
            let c = normalize_quote(c);
            if c == delimiter {
                return Ok(value);
            }
            if c == '\\' {
                let Some(escaped) = self.peek() else {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::InvalidEscape,
                        escape_start..self.pos,
                    ));
                };
                self.pos += escaped.len_utf8();
                let escaped = normalize_quote(escaped);
                if !matches!(escaped, '"' | '\'' | '\\') {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::InvalidEscape,
                        escape_start..self.pos,
                    ));
                }
                value.push(escaped);
            } else {
                value.push(c);
            }
        }
        Err(QueryParseError::new(
            QueryParseErrorKind::UnterminatedQuote,
            start..self.pos,
        ))
    }

    fn word(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace()
                || matches!(c, '(' | ')')
                || self.input[self.pos..].starts_with("/*")
            {
                break;
            }
            self.pos += c.len_utf8();
        }
        self.input[start..self.pos]
            .chars()
            .map(normalize_quote)
            .collect()
    }

    fn filter_or_word(&mut self, start: usize) -> Result<TokenKind, QueryParseError> {
        if self.peek() == Some(':') {
            return Err(QueryParseError::new(
                QueryParseErrorKind::EmptyFilterKey,
                start..start + 1,
            ));
        }
        // Only ASCII alphabetic prefixes followed immediately by ':' are keys.
        let key_end = self.input[self.pos..]
            .bytes()
            .take_while(u8::is_ascii_alphabetic)
            .count()
            + self.pos;
        let is_filter = key_end > self.pos && self.input[key_end..].starts_with(':');
        let key = if is_filter {
            let key = self.input[self.pos..key_end].to_ascii_lowercase();
            self.pos = key_end + 1;
            key
        } else {
            "keyword".to_owned()
        };
        let value = if self
            .peek()
            .is_some_and(|c| matches!(normalize_quote(c), '"' | '\''))
        {
            self.quoted(start)?
        } else {
            self.word()
        };
        if value.is_empty() {
            return Err(QueryParseError::new(
                QueryParseErrorKind::EmptyFilterValue,
                start..self.pos,
            ));
        }
        if !is_filter {
            match value.to_ascii_lowercase().as_str() {
                "and" => return Ok(TokenKind::And),
                "or" => return Ok(TokenKind::Or),
                "not" => return Ok(TokenKind::Not),
                _ => {}
            }
        }
        Ok(TokenKind::Filter { key, value })
    }
}

pub(super) fn lex(input: &str) -> Result<Vec<Token>, QueryParseError> {
    let mut lexer = Lexer { input, pos: 0 };
    let mut tokens = Vec::new();
    let mut separated = false;
    while let Some(c) = lexer.peek() {
        let start = lexer.pos;
        if c.is_whitespace() {
            lexer.pos += c.len_utf8();
            separated = true;
            continue;
        }
        if input[start..].starts_with("/*") {
            let Some(end) = input[start + 2..].find("*/") else {
                return Err(QueryParseError::new(
                    QueryParseErrorKind::UnterminatedComment,
                    start..input.len(),
                ));
            };
            lexer.pos = start + 2 + end + 2;
            separated = true;
            continue;
        }
        let kind = match normalize_quote(c) {
            '(' => {
                lexer.pos += 1;
                TokenKind::Open
            }
            ')' => {
                lexer.pos += 1;
                TokenKind::Close
            }
            '-' => {
                lexer.pos += 1;
                TokenKind::Not
            }
            '"' | '\'' => {
                let value = lexer.quoted(start)?;
                if value.is_empty() {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::EmptyFilterValue,
                        start..lexer.pos,
                    ));
                }
                TokenKind::Filter {
                    key: "keyword".into(),
                    value,
                }
            }
            _ => lexer.filter_or_word(start)?,
        };
        tokens.push(Token {
            kind,
            span: start..lexer.pos,
            separated,
        });
        separated = false;
    }
    Ok(tokens)
}

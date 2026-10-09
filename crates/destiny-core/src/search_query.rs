//! Bounded DIM-style query syntax, typed validation, and pure execution primitives.
//!
//! Spans are half-open byte ranges in the original UTF-8 input. Groups expand
//! their root node's span to include the parentheses; no separate group node is
//! retained. Values preserve case, while curly quotes normalize to straight ones.

mod eval;
mod lexer;

pub use eval::{ItemView, Lowering, lower, matches, referenced_stats};
mod parser;
mod registry;

pub use registry::{
    Comparison, FilterFormat, FilterSpec, Leaf, TypedQuery, TypedQueryKind, ValueKind, compile,
    list_filters,
};

use std::{fmt, ops::Range};

use crate::AppError;

pub const MAX_QUERY_BYTES: usize = 1024;
/// Maximum combined nesting of parentheses and unary negations.
pub const MAX_QUERY_DEPTH: usize = 32;
/// Maximum retained AST nodes, including boolean and unary operators.
pub const MAX_QUERY_NODES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryNode {
    pub kind: QueryNodeKind,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryNodeKind {
    Noop,
    And(Vec<QueryNode>),
    Or(Vec<QueryNode>),
    Not(Box<QueryNode>),
    Filter { key: String, value: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryParseErrorKind {
    UnbalancedParenthesis,
    UnterminatedQuote,
    InvalidEscape,
    UnterminatedComment,
    DanglingOperator,
    EmptyGroup,
    EmptyFilterKey,
    EmptyFilterValue,
    TrailingGarbage,
    QueryTooLong,
    TooDeeplyNested,
    TooManyTerms,
    UnknownFilter,
    UnsupportedFilter,
    InvalidFilterValue,
}

impl QueryParseErrorKind {
    /// Stable machine-readable spelling for future transport diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnbalancedParenthesis => "unbalanced_parenthesis",
            Self::UnterminatedQuote => "unterminated_quote",
            Self::InvalidEscape => "invalid_escape",
            Self::UnterminatedComment => "unterminated_comment",
            Self::DanglingOperator => "dangling_operator",
            Self::EmptyGroup => "empty_group",
            Self::EmptyFilterKey => "empty_filter_key",
            Self::EmptyFilterValue => "empty_filter_value",
            Self::TrailingGarbage => "trailing_garbage",
            Self::QueryTooLong => "query_too_long",
            Self::TooDeeplyNested => "too_deeply_nested",
            Self::TooManyTerms => "too_many_terms",
            Self::UnknownFilter => "unknown_filter",
            Self::UnsupportedFilter => "unsupported_filter",
            Self::InvalidFilterValue => "invalid_filter_value",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParseError {
    pub kind: QueryParseErrorKind,
    pub span: Range<usize>,
    /// Sanitized English diagnostic: may name a static filter, never echoes input.
    pub message: String,
}

impl QueryParseError {
    fn new(kind: QueryParseErrorKind, span: Range<usize>) -> Self {
        let reason = match kind {
            QueryParseErrorKind::UnbalancedParenthesis => "unbalanced parenthesis",
            QueryParseErrorKind::UnterminatedQuote => "unterminated quoted string",
            QueryParseErrorKind::InvalidEscape => "invalid quoted-string escape",
            QueryParseErrorKind::UnterminatedComment => "unterminated comment",
            QueryParseErrorKind::DanglingOperator => "operator requires an operand",
            QueryParseErrorKind::EmptyGroup => "parenthesized group is empty",
            QueryParseErrorKind::EmptyFilterKey => "filter key is empty",
            QueryParseErrorKind::EmptyFilterValue => "filter value is empty",
            QueryParseErrorKind::TrailingGarbage => "expected an operator or separator",
            QueryParseErrorKind::QueryTooLong => "query exceeds 1024 bytes",
            QueryParseErrorKind::TooDeeplyNested => "query exceeds nesting depth 32",
            QueryParseErrorKind::TooManyTerms => "query exceeds 128 AST nodes",
            QueryParseErrorKind::UnknownFilter => "unknown filter",
            QueryParseErrorKind::UnsupportedFilter => "unsupported filter",
            QueryParseErrorKind::InvalidFilterValue => "invalid filter value",
        };
        let message = format!("{reason} at bytes {}..{}", span.start, span.end);
        Self {
            kind,
            span,
            message,
        }
    }

    pub fn to_app_error(&self) -> AppError {
        AppError::invalid_argument("query", &self.message)
    }
}

impl From<QueryParseError> for AppError {
    fn from(error: QueryParseError) -> Self {
        error.to_app_error()
    }
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for QueryParseError {}

/// Parse syntax only. Unknown filter names and values are left for the registry.
/// Empty or comment-only input yields `Noop` with span `0..0`.
pub fn parse_query(query: &str) -> Result<QueryNode, QueryParseError> {
    if query.len() > MAX_QUERY_BYTES {
        // Keep the offending span on UTF-8 boundaries even if byte 1024 is
        // inside a multibyte character.
        let mut start = MAX_QUERY_BYTES;
        while !query.is_char_boundary(start) {
            start -= 1;
        }
        return Err(QueryParseError::new(
            QueryParseErrorKind::QueryTooLong,
            start..query.len(),
        ));
    }
    parser::parse(lexer::lex(query)?)
}

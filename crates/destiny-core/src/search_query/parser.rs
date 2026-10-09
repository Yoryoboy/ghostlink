use super::lexer::{Token, TokenKind};
use super::{
    MAX_QUERY_DEPTH, MAX_QUERY_NODES, QueryNode, QueryNodeKind, QueryParseError,
    QueryParseErrorKind,
};

struct Parser {
    tokens: std::vec::IntoIter<Token>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.as_slice().first()
    }

    fn atom(&mut self, depth: usize) -> Result<QueryNode, QueryParseError> {
        let token = self.tokens.next().expect("caller checked for an atom");
        let span = token.span;
        let kind = match token.kind {
            TokenKind::Filter { key, value } => {
                if key == "not" {
                    self.check_depth(depth, &span)?;
                    QueryNodeKind::Not(Box::new(QueryNode {
                        kind: QueryNodeKind::Filter {
                            key: "is".into(),
                            value,
                        },
                        span: span.clone(),
                    }))
                } else {
                    QueryNodeKind::Filter { key, value }
                }
            }
            TokenKind::Not => {
                self.check_depth(depth, &span)?;
                self.require_operand(&span)?;
                let child = self.atom(depth + 1)?;
                return Ok(QueryNode {
                    span: span.start..child.span.end,
                    kind: QueryNodeKind::Not(Box::new(child)),
                });
            }
            TokenKind::Open => {
                self.check_depth(depth, &span)?;
                if self
                    .peek()
                    .is_some_and(|t| matches!(t.kind, TokenKind::Close))
                {
                    let close = self.tokens.next().expect("peeked close");
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::EmptyGroup,
                        span.start..close.span.end,
                    ));
                }
                if self.peek().is_none() {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::UnbalancedParenthesis,
                        span,
                    ));
                }
                let mut child = self.expression(1, depth + 1)?;
                let Some(close) = self.tokens.next() else {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::UnbalancedParenthesis,
                        span,
                    ));
                };
                if !matches!(close.kind, TokenKind::Close) {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::UnbalancedParenthesis,
                        close.span,
                    ));
                }
                child.span = span.start..close.span.end;
                return Ok(child);
            }
            TokenKind::Close => {
                return Err(QueryParseError::new(
                    QueryParseErrorKind::UnbalancedParenthesis,
                    span,
                ));
            }
            TokenKind::And | TokenKind::Or => {
                return Err(QueryParseError::new(
                    QueryParseErrorKind::DanglingOperator,
                    span,
                ));
            }
        };
        Ok(QueryNode { kind, span })
    }

    fn check_depth(
        &self,
        depth: usize,
        span: &std::ops::Range<usize>,
    ) -> Result<(), QueryParseError> {
        if depth >= MAX_QUERY_DEPTH {
            Err(QueryParseError::new(
                QueryParseErrorKind::TooDeeplyNested,
                span.clone(),
            ))
        } else {
            Ok(())
        }
    }

    fn require_operand(&self, span: &std::ops::Range<usize>) -> Result<(), QueryParseError> {
        if self
            .peek()
            .is_none_or(|t| matches!(t.kind, TokenKind::Close | TokenKind::And | TokenKind::Or))
        {
            Err(QueryParseError::new(
                QueryParseErrorKind::DanglingOperator,
                span.clone(),
            ))
        } else {
            Ok(())
        }
    }

    // DIM precedence climbing: implicit AND=1, OR=2, explicit AND=3.
    // Parsing the RHS at precedence+1 makes each binary operator left-associative.
    fn expression(&mut self, minimum: u8, depth: usize) -> Result<QueryNode, QueryParseError> {
        let mut lhs = self.atom(depth)?;
        while let Some(token) = self.peek() {
            let (precedence, is_and, explicit) = match token.kind {
                TokenKind::Close => break,
                TokenKind::And => (3, true, true),
                TokenKind::Or => (2, false, true),
                _ if token.separated => (1, true, false),
                _ => {
                    return Err(QueryParseError::new(
                        QueryParseErrorKind::TrailingGarbage,
                        token.span.clone(),
                    ));
                }
            };
            if precedence < minimum {
                break;
            }
            if explicit {
                let operator = self.tokens.next().expect("peeked operator");
                self.require_operand(&operator.span)?;
            }
            let rhs = self.expression(precedence + 1, depth)?;
            let span = lhs.span.start..rhs.span.end;
            // Preserve DIM's asymmetric flattening: an existing LHS list appends
            // the RHS whole; otherwise a same-kind RHS list is spread.
            match (&mut lhs.kind, is_and) {
                (QueryNodeKind::And(children), true) | (QueryNodeKind::Or(children), false) => {
                    children.push(rhs)
                }
                _ => {
                    let mut children = vec![lhs];
                    match (rhs.kind, is_and) {
                        (QueryNodeKind::And(more), true) | (QueryNodeKind::Or(more), false) => {
                            children.extend(more)
                        }
                        (kind, _) => children.push(QueryNode {
                            kind,
                            span: rhs.span,
                        }),
                    }
                    lhs = QueryNode {
                        kind: if is_and {
                            QueryNodeKind::And(children)
                        } else {
                            QueryNodeKind::Or(children)
                        },
                        span: span.clone(),
                    };
                }
            }
            lhs.span = span;
        }
        Ok(lhs)
    }
}

pub(super) fn parse(tokens: Vec<Token>) -> Result<QueryNode, QueryParseError> {
    let mut parser = Parser {
        tokens: tokens.into_iter(),
    };
    if parser.peek().is_none() {
        return Ok(QueryNode {
            kind: QueryNodeKind::Noop,
            span: 0..0,
        });
    }
    let node = parser.expression(1, 0)?;
    if let Some(token) = parser.tokens.next() {
        return Err(QueryParseError::new(
            QueryParseErrorKind::UnbalancedParenthesis,
            token.span,
        ));
    }
    // Count retained nodes rather than discarded intermediate binary/group nodes.
    // Input bytes and syntactic depth already bound allocation and recursion.
    let mut pending = vec![&node];
    let mut count = 0;
    while let Some(current) = pending.pop() {
        count += 1;
        if count > MAX_QUERY_NODES {
            return Err(QueryParseError::new(
                QueryParseErrorKind::TooManyTerms,
                current.span.clone(),
            ));
        }
        match &current.kind {
            QueryNodeKind::And(children) | QueryNodeKind::Or(children) => {
                pending.extend(children.iter().rev())
            }
            QueryNodeKind::Not(child) => pending.push(child),
            _ => {}
        }
    }
    Ok(node)
}

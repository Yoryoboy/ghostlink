use destiny_core::search_query::{QueryNode, QueryNodeKind, QueryParseErrorKind, parse_query};
use destiny_core::{AppError, ErrorCategory};

fn sexpr(node: &QueryNode) -> String {
    match &node.kind {
        QueryNodeKind::Noop => "noop".into(),
        QueryNodeKind::Filter { key, value } => format!("{key}:{value}"),
        QueryNodeKind::Not(child) => format!("(not {})", sexpr(child)),
        QueryNodeKind::And(children) | QueryNodeKind::Or(children) => {
            let op = if matches!(node.kind, QueryNodeKind::And(_)) {
                "and"
            } else {
                "or"
            };
            format!(
                "({op} {})",
                children.iter().map(sexpr).collect::<Vec<_>>().join(" ")
            )
        }
    }
}

#[test]
fn precedence_matrix_and_left_associativity() {
    for (query, expected) in [
        (
            "a b or c and d",
            "(and keyword:a (or keyword:b (and keyword:c keyword:d)))",
        ),
        ("a or b c", "(and (or keyword:a keyword:b) keyword:c)"),
        ("a and b c", "(and keyword:a keyword:b keyword:c)"),
        ("a b and c", "(and keyword:a keyword:b keyword:c)"),
        ("a and b or c", "(or (and keyword:a keyword:b) keyword:c)"),
        ("a or b and c", "(or keyword:a (and keyword:b keyword:c))"),
        ("a or b or c", "(or keyword:a keyword:b keyword:c)"),
        ("a or (b or c)", "(or keyword:a keyword:b keyword:c)"),
        (
            "a or b or (c or d)",
            "(or keyword:a keyword:b (or keyword:c keyword:d))",
        ),
        ("a and b and c", "(and keyword:a keyword:b keyword:c)"),
        ("a b c", "(and keyword:a keyword:b keyword:c)"),
        ("(a or b) and c", "(and (or keyword:a keyword:b) keyword:c)"),
        ("a or (b c)", "(or keyword:a (and keyword:b keyword:c))"),
        ("a (b c)", "(and keyword:a keyword:b keyword:c)"),
        // DIM appends a same-kind RHS without flattening when the LHS already has that kind.
        (
            "a b (c d)",
            "(and keyword:a keyword:b (and keyword:c keyword:d))",
        ),
        ("a OR b AND c", "(or keyword:a (and keyword:b keyword:c))"),
    ] {
        assert_eq!(sexpr(&parse_query(query).unwrap()), expected, "{query}");
    }
}

#[test]
fn negation_forms() {
    for (query, expected) in [
        ("-x", "(not keyword:x)"),
        ("- x", "(not keyword:x)"),
        ("-(a or b)", "(not (or keyword:a keyword:b))"),
        ("not x", "(not keyword:x)"),
        ("NOT x", "(not keyword:x)"),
        ("not:locked", "(not is:locked)"),
        ("--x", "(not (not keyword:x))"),
        (
            "android ordinary notable",
            "(and keyword:android keyword:ordinary keyword:notable)",
        ),
    ] {
        assert_eq!(sexpr(&parse_query(query).unwrap()), expected);
    }
}

#[test]
fn quotes_escapes_case_and_comments() {
    for (query, expected) in [
        ("NAME:\"Ace of Spades\"", "name:Ace of Spades"),
        ("'Ace of Spades'", "keyword:Ace of Spades"),
        (r#""a\"b\'c\\d""#, "keyword:a\"b'c\\d"),
        (r#"'a\"b\'c\\d'"#, "keyword:a\"b'c\\d"),
        ("“Ace of Spades”", "keyword:Ace of Spades"),
        ("name:‘Kings Fall’", "name:Kings Fall"),
        ("a /* ignored \n and is: */ b", "(and keyword:a keyword:b)"),
        ("/* leading */ a /* trailing */", "keyword:a"),
        ("a/*comment*/or/*comment*/b", "(or keyword:a keyword:b)"),
        ("is:ARc", "is:ARc"),
        ("\"not\"", "keyword:not"),
        ("name:\"or\"", "name:or"),
        ("is:not", "is:not"),
        ("stat:mobility:>=20", "stat:mobility:>=20"),
        ("x-y", "keyword:x-y"),
        ("unknown:MixedCase", "unknown:MixedCase"),
    ] {
        assert_eq!(sexpr(&parse_query(query).unwrap()), expected, "{query}");
    }
}

#[test]
fn original_byte_spans_include_groups_and_negation() {
    let node = parse_query("  - (“é” or IS:Arc)  ").unwrap();
    assert_eq!(node.span, 2..24);
    let QueryNodeKind::Not(group) = node.kind else {
        panic!("not")
    };
    assert_eq!(group.span, 4..24);
    let QueryNodeKind::Or(children) = group.kind else {
        panic!("or")
    };
    assert_eq!(children[0].span, 5..13);
    assert_eq!(children[1].span, 17..23);
}

#[test]
fn empty_queries() {
    for query in ["", " \n\t", "/* only a comment */"] {
        let node = parse_query(query).unwrap();
        assert_eq!(node.kind, QueryNodeKind::Noop);
        assert_eq!(node.span, 0..0);
    }
}

#[test]
fn errors_have_stable_kinds_and_exact_spans() {
    use QueryParseErrorKind::*;
    for (query, kind, span) in [
        ("(a", UnbalancedParenthesis, 0..1),
        ("a)", UnbalancedParenthesis, 1..2),
        ("()", EmptyGroup, 0..2),
        ("( /*x*/ )", EmptyGroup, 0..9),
        ("and a", DanglingOperator, 0..3),
        ("or a", DanglingOperator, 0..2),
        ("a and or b", DanglingOperator, 2..5),
        ("not and x", DanglingOperator, 0..3),
        ("a and", DanglingOperator, 2..5),
        ("a or", DanglingOperator, 2..4),
        ("not", DanglingOperator, 0..3),
        ("-  ", DanglingOperator, 0..1),
        ("(a or)", DanglingOperator, 3..5),
        (":x", EmptyFilterKey, 0..1),
        ("is:", EmptyFilterValue, 0..3),
        ("is: x", EmptyFilterValue, 0..3),
        ("is:\"\"", EmptyFilterValue, 0..5),
        ("\"\"", EmptyFilterValue, 0..2),
        ("is:()", EmptyFilterValue, 0..3),
        ("\"abc", UnterminatedQuote, 0..4),
        ("name:\"abc", UnterminatedQuote, 0..9),
        ("‘é", UnterminatedQuote, 0..5),
        (r#""a\n""#, InvalidEscape, 2..4),
        ("\"a\\", InvalidEscape, 2..3),
        ("/* abc", UnterminatedComment, 0..6),
        ("\"a\"b", TrailingGarbage, 3..4),
        ("(a)b", TrailingGarbage, 3..4),
    ] {
        let error = parse_query(query).unwrap_err();
        assert_eq!(error.kind, kind, "{query}");
        assert_eq!(error.span, span, "{query}");
        assert!(error.message.contains("bytes"));
        assert!(!error.message.contains('\n'));
        assert!(!error.kind.as_str().is_empty());
        assert_eq!(error.to_string(), error.message);
        let app = AppError::from(error.clone());
        assert_eq!(app.category(), ErrorCategory::ProviderValidation);
        assert_eq!(app, error.to_app_error());
    }
}

#[test]
fn bounds_at_limit_and_limit_plus_one() {
    use QueryParseErrorKind::*;
    assert!(parse_query(&"x".repeat(1024)).is_ok());
    let error = parse_query(&"x".repeat(1025)).unwrap_err();
    assert_eq!((error.kind, error.span), (QueryTooLong, 1024..1025));
    assert!(parse_query(&"é".repeat(512)).is_ok());
    let error = parse_query(&"é".repeat(513)).unwrap_err();
    assert_eq!((error.kind, error.span), (QueryTooLong, 1024..1026));
    let error = parse_query(&format!("{}é", "x".repeat(1023))).unwrap_err();
    assert_eq!((error.kind, error.span), (QueryTooLong, 1023..1025));
    assert!(parse_query(&format!("{}x{}", "(".repeat(32), ")".repeat(32))).is_ok());
    let error = parse_query(&format!("{}x{}", "(".repeat(33), ")".repeat(33))).unwrap_err();
    assert_eq!((error.kind, error.span), (TooDeeplyNested, 32..33));
    assert!(parse_query(&format!("{}x", "-".repeat(32))).is_ok());
    let error = parse_query(&format!("{}x", "-".repeat(33))).unwrap_err();
    assert_eq!((error.kind, error.span), (TooDeeplyNested, 32..33));
    // The bound counts all retained AST nodes, including boolean operators.
    let at_limit = vec!["x"; 127].join(" ");
    assert!(parse_query(&at_limit).is_ok());
    let over_limit = vec!["x"; 128].join(" ");
    let error = parse_query(&over_limit).unwrap_err();
    assert_eq!((error.kind, error.span), (TooManyTerms, 254..255));
}

#[test]
fn short_syntax_combinations_never_panic_and_keep_valid_spans() {
    fn check_node(query: &str, node: &QueryNode) {
        assert!(query.get(node.span.clone()).is_some());
        match &node.kind {
            QueryNodeKind::And(children) | QueryNodeKind::Or(children) => {
                for child in children {
                    assert!(node.span.start <= child.span.start);
                    assert!(node.span.end >= child.span.end);
                    check_node(query, child);
                }
            }
            QueryNodeKind::Not(child) => check_node(query, child),
            _ => {}
        }
    }
    let fragments = [
        "a", " ", "(", ")", "-", "not", "and", "or", "“", "é", "\\", "/*x*/", ":",
    ];
    for a in fragments {
        for b in fragments {
            for c in fragments {
                let query = format!("{a}{b}{c}");
                match parse_query(&query) {
                    Ok(node) => check_node(&query, &node),
                    Err(error) => assert!(query.get(error.span).is_some(), "{query}"),
                }
            }
        }
    }
}

#[test]
fn realistic_dim_query_preserves_filter_values() {
    for (query, expected) in [
        (
            "is:weapon is:arc perk:rampage -is:locked (is:legendary or is:exotic) stat:mobility:>=20 power:>2000 \"ace of spades\"",
            "(and is:weapon is:arc perk:rampage (not is:locked) (or is:legendary is:exotic) stat:mobility:>=20 power:>2000 keyword:ace of spades)",
        ),
        (
            "is:armor stat:resilience:>=30 not:locked",
            "(and is:armor stat:resilience:>=30 (not is:locked))",
        ),
        (
            "(is:solar or is:void) -power:>2000",
            "(and (or is:solar is:void) (not power:>2000))",
        ),
        ("exactname:'Ace of Spades'", "exactname:Ace of Spades"),
    ] {
        assert_eq!(sexpr(&parse_query(query).unwrap()), expected, "{query}");
    }
}

#[test]
fn machine_readable_error_kind_spellings_are_stable() {
    use QueryParseErrorKind::*;
    for (kind, expected) in [
        (UnbalancedParenthesis, "unbalanced_parenthesis"),
        (UnterminatedQuote, "unterminated_quote"),
        (InvalidEscape, "invalid_escape"),
        (UnterminatedComment, "unterminated_comment"),
        (DanglingOperator, "dangling_operator"),
        (EmptyGroup, "empty_group"),
        (EmptyFilterKey, "empty_filter_key"),
        (EmptyFilterValue, "empty_filter_value"),
        (TrailingGarbage, "trailing_garbage"),
        (QueryTooLong, "query_too_long"),
        (TooDeeplyNested, "too_deeply_nested"),
        (TooManyTerms, "too_many_terms"),
    ] {
        assert_eq!(kind.as_str(), expected);
    }
}

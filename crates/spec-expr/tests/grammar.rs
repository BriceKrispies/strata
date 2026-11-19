//! The grammar of `src/core/expr.js`: what parses to which tree, and what is rejected.

use strata_spec_expr::parse;

fn sexpr(src: &str) -> String {
    parse(src)
        .unwrap_or_else(|e| panic!("`{src}` should parse: {e}"))
        .to_sexpr()
}

fn rejected(src: &str) -> (String, usize) {
    let e = parse(src).expect_err(&format!("`{src}` should be rejected"));
    (e.message, e.pos)
}

#[test]
fn literals() {
    assert_eq!(sexpr("42"), "42");
    assert_eq!(sexpr("1.5"), "1.5");
    assert_eq!(sexpr("'x'"), "\"x\"");
    assert_eq!(sexpr("\"y\""), "\"y\"");
    assert_eq!(sexpr("\"it's\""), "\"it's\"");
    assert_eq!(sexpr("''"), "\"\"");
    assert_eq!(sexpr("true"), "true");
    assert_eq!(sexpr("false"), "false");
    assert_eq!(sexpr("null"), "null");
}

#[test]
fn references() {
    assert_eq!(sexpr("token"), "token");
    assert_eq!(sexpr("event.token"), "event.token");
    assert_eq!(sexpr("lease.count.jl-done"), "lease.count.jl-done");
    assert_eq!(sexpr("a.true"), "a.true");
    assert_eq!(sexpr("_x1"), "_x1");
}

#[test]
fn hyphen_in_identifiers() {
    assert_eq!(sexpr("a-b"), "a-b");
    assert_eq!(sexpr("a-b-c"), "a-b-c");
    assert_eq!(sexpr("a - b"), "(- a b)");
    assert_eq!(sexpr("a -b"), "(- a b)");
    assert_eq!(sexpr("a-1"), "(- a 1)");
    assert_eq!(sexpr("a- b"), "(- a b)");
    assert_eq!(sexpr("a--b"), "(- a (- b))");
}

#[test]
fn precedence_and_associativity() {
    assert_eq!(sexpr("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(sexpr("1 * 2 + 3"), "(+ (* 1 2) 3)");
    assert_eq!(sexpr("1 - 2 - 3"), "(- (- 1 2) 3)");
    assert_eq!(sexpr("8 / 4 % 3"), "(% (/ 8 4) 3)");
    assert_eq!(sexpr("a == b < c"), "(== a (< b c))");
    assert_eq!(sexpr("a < b + 1"), "(< a (+ b 1))");
    assert_eq!(sexpr("a != b == c"), "(== (!= a b) c)");
    assert_eq!(sexpr("a <= b"), "(<= a b)");
    assert_eq!(sexpr("a > b"), "(> a b)");
    assert_eq!(sexpr("a >= b"), "(>= a b)");
    assert_eq!(sexpr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(sexpr("a && b && c"), "(&& (&& a b) c)");
    assert_eq!(sexpr("a || b || c"), "(|| (|| a b) c)");
    assert_eq!(sexpr("a => b => c"), "(=> a (=> b c))");
    assert_eq!(sexpr("a || b => c"), "(=> (|| a b) c)");
    assert_eq!(sexpr("a => b || c"), "(=> a (|| b c))");
}

#[test]
fn unary_and_grouping() {
    assert_eq!(sexpr("!a"), "(! a)");
    assert_eq!(sexpr("-a"), "(- a)");
    assert_eq!(sexpr("- 1"), "(- 1)");
    assert_eq!(sexpr("!!a"), "(! (! a))");
    assert_eq!(sexpr("!-a"), "(! (- a))");
    assert_eq!(sexpr("-a * b"), "(* (- a) b)");
    assert_eq!(sexpr("!a && b"), "(&& (! a) b)");
    assert_eq!(sexpr("(a + b) * c"), "(* (+ a b) c)");
    assert_eq!(sexpr("((a))"), "a");
    assert_eq!(sexpr("1 - -2"), "(- 1 (- 2))");
    assert_eq!(sexpr("a=>b"), "(=> a b)");
}

#[test]
fn real_guards() {
    assert_eq!(
        sexpr("event.token == 'good' && event.browser == 'requester' && time - issuedAt <= 15"),
        "(&& (&& (== event.token \"good\") (== event.browser \"requester\")) (<= (- time issuedAt) 15))"
    );
    assert_eq!(sexpr("token + 1"), "(+ token 1)");
}

#[test]
fn whitespace_is_space_tab_newline_return() {
    assert_eq!(sexpr(" \t\r\na\n+\t1 "), "(+ a 1)");
}

#[test]
fn spans_cover_the_source_text() {
    let e = parse("  a + b * 2 ").unwrap();
    assert_eq!(e.text(), "a + b * 2");
}

#[test]
fn rejects_empty_and_truncated_input() {
    assert_eq!(rejected(""), ("Unexpected end of expression".into(), 0));
    assert_eq!(rejected("   "), ("Unexpected end of expression".into(), 3));
    assert_eq!(rejected("1 +"), ("Unexpected end of expression".into(), 3));
    assert_eq!(rejected("a ||"), ("Unexpected end of expression".into(), 4));
    assert_eq!(rejected("!"), ("Unexpected end of expression".into(), 1));
    assert_eq!(rejected("-"), ("Unexpected end of expression".into(), 1));
    assert_eq!(rejected("("), ("Unexpected end of expression".into(), 1));
}

#[test]
fn rejects_unbalanced_parentheses() {
    assert_eq!(rejected("(1"), ("Expected \")\"".into(), 2));
    assert_eq!(rejected("(1 + 2"), ("Expected \")\"".into(), 6));
    assert_eq!(rejected("1)"), ("Unexpected \")\"".into(), 1));
    assert_eq!(rejected("()"), ("Unexpected \")\"".into(), 1));
}

#[test]
fn rejects_stray_tokens() {
    assert_eq!(rejected("a b"), ("Unexpected \"b\"".into(), 2));
    assert_eq!(rejected("1 2"), ("Unexpected \"2\"".into(), 2));
    assert_eq!(rejected("* 2"), ("Unexpected \"*\"".into(), 0));
    assert_eq!(rejected("=="), ("Unexpected \"==\"".into(), 0));
    assert_eq!(rejected("a + * b"), ("Unexpected \"*\"".into(), 4));
    assert_eq!(rejected("'x' 'y'"), ("Unexpected \"y\"".into(), 4));
    assert_eq!(rejected("1.5.2"), ("Unexpected \".\"".into(), 3));
    assert_eq!(rejected("1."), ("Unexpected \".\"".into(), 1));
    assert_eq!(rejected("true.x"), ("Unexpected \".\"".into(), 4));
    assert_eq!(rejected("1abc"), ("Unexpected \"abc\"".into(), 1));
    assert_eq!(
        rejected("a => "),
        ("Unexpected end of expression".into(), 5)
    );
}

#[test]
fn rejects_bad_references() {
    assert_eq!(rejected("a."), ("Expected a name after \".\"".into(), 2));
    assert_eq!(rejected("a.1"), ("Expected a name after \".\"".into(), 2));
    assert_eq!(rejected("a.(b)"), ("Expected a name after \".\"".into(), 2));
    assert_eq!(rejected("a..b"), ("Expected a name after \".\"".into(), 2));
    assert_eq!(rejected(".a"), ("Unexpected \".\"".into(), 0));
}

#[test]
fn rejects_unterminated_strings() {
    assert_eq!(rejected("'abc"), ("Unterminated string literal".into(), 0));
    assert_eq!(
        rejected("a == \"x"),
        ("Unterminated string literal".into(), 5)
    );
    assert_eq!(rejected("'a\""), ("Unterminated string literal".into(), 0));
}

#[test]
fn rejects_unknown_characters() {
    assert_eq!(rejected("a & b"), ("Unexpected character '&'".into(), 2));
    assert_eq!(rejected("a | b"), ("Unexpected character '|'".into(), 2));
    assert_eq!(rejected("a = b"), ("Unexpected character '='".into(), 2));
    assert_eq!(rejected("a ? b"), ("Unexpected character '?'".into(), 2));
    assert_eq!(rejected("{a}"), ("Unexpected character '{'".into(), 0));
    assert_eq!(rejected("a[0]"), ("Unexpected character '['".into(), 1));
    assert_eq!(rejected("1 , 2"), ("Unexpected character ','".into(), 2));
    assert_eq!(rejected("é"), ("Unexpected character 'é'".into(), 0));
    assert_eq!(rejected("1e3"), ("Unexpected \"e3\"".into(), 1));
    assert_eq!(
        rejected("a == 'x' # y"),
        ("Unexpected character '#'".into(), 9)
    );
}

#[test]
fn deep_nesting_is_refused_not_overflowed() {
    let src = format!("{}1{}", "(".repeat(500), ")".repeat(500));
    assert_eq!(rejected(&src).0, "Expression is nested too deeply");
    let src = format!("{}1{}", "(".repeat(50), ")".repeat(50));
    assert_eq!(sexpr(&src), "1");
}

#[test]
fn errors_render_with_source_and_caret() {
    let e = parse("a & b").unwrap_err();
    assert_eq!(e.source, "a & b");
    let text = e.to_string();
    assert!(text.contains("Unexpected character '&'"), "{text}");
    assert!(text.contains("a & b"), "{text}");
    assert!(text.ends_with("  |   ^"), "{text}");
}

#[test]
fn long_operator_chains_and_prefix_chains_are_bounded() {
    let chain = format!("a{}", " + 1".repeat(150));
    assert_eq!(rejected(&chain).0, "Expression is nested too deeply");
    let prefix = format!("{}a", "!".repeat(150));
    assert_eq!(rejected(&prefix).0, "Expression is nested too deeply");
    let right = format!("{}a", "a => ".repeat(150));
    assert_eq!(rejected(&right).0, "Expression is nested too deeply");
    assert_eq!(
        sexpr(&format!("a{}", " + 1".repeat(50)))
            .matches('+')
            .count(),
        50
    );
}

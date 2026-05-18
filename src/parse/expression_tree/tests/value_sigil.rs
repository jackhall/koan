//! `value_sigil` parse cases for `expression_tree::parse`.


use super::{top, tree};

#[test]
fn quote_sigil_wraps_body_in_quote_keyword() {
    // `#(foo)` desugars to `(QUOTE (foo))` — the body is always nested in an inner
    // Expression so the QUOTE builtin's `KExpression`-typed slot accepts it.
    assert_eq!(tree("#(foo)").unwrap(), "[[t(QUOTE) [t(foo)]]]");
}

#[test]
fn eval_sigil_wraps_body_in_eval_keyword() {
    assert_eq!(tree("$(x)").unwrap(), "[[t(EVAL) [t(x)]]]");
}

#[test]
fn quote_sigil_preserves_multi_part_inner() {
    assert_eq!(tree("#(a b c)").unwrap(), "[[t(QUOTE) [t(a) t(b) t(c)]]]");
}

#[test]
fn nested_sigils_quote_around_eval() {
    // `$(#(x))` — the outer EVAL wraps an expression that itself is `(QUOTE x)`.
    assert_eq!(tree("$(#(x))").unwrap(), "[[t(EVAL) [[t(QUOTE) [t(x)]]]]]");
}

#[test]
fn quote_sigil_inside_list_literal() {
    assert_eq!(
        tree("[a #(b) c]").unwrap(),
        "[L[t(a) [t(QUOTE) [t(b)]] t(c)]]",
    );
}

#[test]
fn quote_sigil_as_dict_value() {
    assert_eq!(
        tree("{x: #(y)}").unwrap(),
        "[D{t(x): [t(QUOTE) [t(y)]]}]",
    );
}

#[test]
fn eval_sigil_as_call_argument() {
    // `PRINT $(x)` — the EVAL form is the second part of the PRINT call, just like a
    // parenthesized sub-expression would be.
    assert_eq!(tree("PRINT $(x)").unwrap(), "[t(PRINT) [t(EVAL) [t(x)]]]");
}

#[test]
fn quote_sigil_without_paren_errors() {
    // `#foo` — the surface is paren-only.
    assert!(tree("#foo").is_err());
}

#[test]
fn eval_sigil_without_paren_errors() {
    assert!(tree("$x").is_err());
}

#[test]
fn quote_sigil_with_whitespace_before_paren_errors() {
    // `# (foo)` — whitespace breaks the contiguity rule.
    assert!(tree("# (foo)").is_err());
}

#[test]
fn quote_sigil_followed_by_number_errors() {
    assert!(tree("#42").is_err());
}

#[test]
fn quote_sigil_followed_by_close_brace_errors() {
    assert!(tree("#}").is_err());
}

#[test]
fn double_sigil_errors() {
    // `#$x` — second sigil arrives while the first is still pending.
    assert!(tree("#$x").is_err());
    assert!(tree("#$(x)").is_err());
}

#[test]
fn trailing_sigil_at_end_of_input_errors() {
    assert!(tree("#").is_err());
    assert!(tree("$").is_err());
}

#[test]
fn comma_continuation_with_bare_sigil_parse_errors() {
    let err = top("add 1,\n  #2").unwrap_err();
    assert_eq!(err, "expected '(' after '#', found '2'");
}

#[test]
fn comma_continuation_with_paren_sigil_parses() {
    // Multi-line form matches the inline form `add 1, #(2)` exactly — the comma is a
    // no-op inside the expression frame and the sigil desugars to `(QUOTE (2))`.
    assert_eq!(
        top("add 1,\n  #(2)").unwrap(),
        top("add 1, #(2)").unwrap(),
    );
    assert_eq!(
        top("add 1,\n  #(2)").unwrap(),
        vec!["[t(add) n(1) [t(QUOTE) [n(2)]]]"],
    );
}

#[test]
fn bracket_continuation_with_bare_sigil_parse_errors() {
    let err = top("LET xs = [\n  #3\n]").unwrap_err();
    assert_eq!(err, "expected '(' after '#', found '3'");
}

#[test]
fn bracket_continuation_with_paren_sigils_parses_to_quote_list() {
    // List literal carries two QUOTE expressions — each `#(n)` desugared independently.
    assert_eq!(
        top("LET xs = [\n  #(3)\n  #(4)\n]").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[[t(QUOTE) [n(3)]] [t(QUOTE) [n(4)]]]]"],
    );
}

#[test]
fn dict_continuation_with_paren_sigils_parses_to_quote_values() {
    // Dict-as-struct shape from the roadmap: each value is a `#(...)` QUOTE that a
    // struct constructor will dispatch on later.
    assert_eq!(
        top("LET d = {\n  x: #(foo)\n  y: #(bar)\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) D{t(x): [t(QUOTE) [t(foo)]], t(y): [t(QUOTE) [t(bar)]]}]"],
    );
}

//! `basics` parse cases for `expression_tree::parse`.


use super::{top, tree};

#[test]
fn parse_single_line_has_no_top_level_wrapper() {
    assert_eq!(top("foo bar").unwrap(), vec!["[t(foo) t(bar)]"]);
}

#[test]
fn parse_multiple_lines_are_siblings() {
    assert_eq!(top("foo\nbar").unwrap(), vec!["[t(foo)]", "[t(bar)]"]);
}

#[test]
fn parse_peels_top_level_redundant_parens() {
    assert_eq!(top("(foo bar)").unwrap(), top("foo bar").unwrap());
}

#[test]
fn parse_peels_multiple_redundant_layers() {
    assert_eq!(top("(((foo bar)))").unwrap(), vec!["[t(foo) t(bar)]"]);
}

#[test]
fn parse_peels_redundant_wrappers_inside_subexpressions() {
    // The inner `((bar baz))` collapses to `(bar baz)` — a sub-expression with one
    // wrapping layer, not two — so peel doesn't change argument arity.
    assert_eq!(
        top("foo ((bar baz))").unwrap(),
        top("foo (bar baz)").unwrap(),
    );
}

#[test]
fn parse_keeps_meaningful_subexpression_parens() {
    // A single set of parens around an argument is meaningful structure, not redundancy.
    assert_eq!(
        top("foo (bar baz)").unwrap(),
        vec!["[t(foo) [t(bar) t(baz)]]"],
    );
}

#[test]
fn empty_input() {
    assert_eq!(tree("").unwrap(), "[]");
}

#[test]
fn single_token() {
    assert_eq!(tree("foo").unwrap(), "[t(foo)]");
}

#[test]
fn split_on_whitespace() {
    assert_eq!(tree("hi there").unwrap(), "[t(hi) t(there)]");
}

#[test]
fn runs_of_whitespace_collapse() {
    assert_eq!(tree("  hi   there  ").unwrap(), "[t(hi) t(there)]");
}

#[test]
fn empty_parens() {
    assert_eq!(tree("()").unwrap(), "[[]]");
}

#[test]
fn flat_parens() {
    assert_eq!(tree("(hi there)").unwrap(), "[[t(hi) t(there)]]");
}

#[test]
fn siblings_and_groups() {
    assert_eq!(
        tree("hey (whoever you are) look at").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at)]"
    );
}

#[test]
fn two_paren_groups() {
    assert_eq!(
        tree("hey (whoever you are) look at (that over there)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(that) t(over) t(there)]]"
    );
}

#[test]
fn nested_parens() {
    assert_eq!(
        tree("hey (whoever you are) look at (whatever (that over there) is)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(whatever) [t(that) t(over) t(there)] t(is)]]"
    );
}

#[test]
fn adjacent_paren_groups() {
    assert_eq!(
        tree("hey (whoever you are)(hello in this language)").unwrap(),
        "[t(hey) [t(whoever) t(you) t(are)] [t(hello) t(in) t(this) t(language)]]"
    );
}

#[test]
fn deeply_nested() {
    assert_eq!(
        tree("hey (whoever (i think) you are (when i remember) now) look at").unwrap(),
        "[t(hey) [t(whoever) [t(i) t(think)] t(you) t(are) [t(when) t(i) t(remember)] t(now)] t(look) t(at)]"
    );
}

#[test]
fn close_without_open_errors() {
    assert!(tree(")(").is_err());
    assert!(tree("has closed) paren only").is_err());
    assert!(tree("two (closed one) open)").is_err());
}

#[test]
fn open_without_close_errors() {
    assert!(tree("has (open paren only").is_err());
    assert!(tree("(two (open one closed)").is_err());
}

#[test]
fn single_pair_dict() {
    assert_eq!(tree("{a: 1}").unwrap(), "[D{t(a): n(1)}]");
}

#[test]
fn multi_part_value_in_parens() {
    assert_eq!(
        tree("{a: (foo bar)}").unwrap(),
        "[D{t(a): [t(foo) t(bar)]}]",
    );
}

#[test]
fn multi_part_value_without_parens_errors() {
    // `{a: foo bar}` parses key=`a`, value=`foo`, auto-commits — then `bar` starts a new
    // key and `}` closes with that key unterminated. The constraint is intentional:
    // dict values are single-token unless parenthesized, mirroring list elements.
    assert!(tree("{a: foo bar}").is_err());
}

#[test]
fn sigil_glued_to_token_errors() {
    // `foo#(...)` — the sigil mid-token isn't allowed. Surfaces as an error at the
    // sigil site rather than later.
    assert!(tree("foo#(x)").is_err());
}

// --- Sigils on continuation lines that bypass the wrap-operand fix ---
//
// The `collapse_whitespace` wrap-outside-paren rewrite only runs on the indent-driven
// path. Comma-continuation and bracket/dict-continuation lines append verbatim — a bare
// `#sym` survives unchanged and `build_tree` rejects it under the sigil-adjacency rule.
// The companion collapse-output assertions live in `whitespace.rs::tests`; the cases
// below lock the end-to-end parse contract (error messages, AST shape).

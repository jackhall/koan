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
    assert_eq!(
        top("foo ((bar baz))").unwrap(),
        top("foo (bar baz)").unwrap(),
    );
}

#[test]
fn parse_keeps_meaningful_subexpression_parens() {
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
    // Dict values are single-token unless parenthesized, mirroring list elements.
    assert!(tree("{a: foo bar}").is_err());
}

#[test]
fn sigil_glued_to_token_errors() {
    assert!(tree("foo#(x)").is_err());
}

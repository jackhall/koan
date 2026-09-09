//! Layout regression net: one test per input the layout pass is responsible for, asserting
//! the statement shapes `top` produces. Each expectation is a recorded shape rather than a
//! derived one, so this file is the arbiter for every layout question the lowering answers —
//! line nesting, the three continuation regimes, sigil-led lines, and the peel.

use super::top;

// --- Lines and indentation ---

#[test]
fn empty_input() {
    assert_eq!(top("").unwrap(), Vec::<String>::new());
}

#[test]
fn only_whitespace() {
    assert_eq!(top("   \n\t\n   \n").unwrap(), Vec::<String>::new());
}

#[test]
fn single_line() {
    assert_eq!(top("foo").unwrap(), vec!["[t(foo)]"]);
}

#[test]
fn single_line_multiple_tokens() {
    assert_eq!(top("foo bar baz").unwrap(), vec!["[t(foo) t(bar) t(baz)]"]);
}

#[test]
fn sibling_lines() {
    assert_eq!(top("foo\nbar").unwrap(), vec!["[t(foo)]", "[t(bar)]",],);
}

#[test]
fn parent_with_child() {
    assert_eq!(top("foo\n    bar").unwrap(), vec!["[t(foo) [t(bar)]]"]);
}

#[test]
fn parent_with_two_children() {
    assert_eq!(
        top("foo\n    bar\n    baz").unwrap(),
        vec!["[t(foo) [t(bar)] [t(baz)]]"]
    );
}

#[test]
fn nested_three_deep() {
    assert_eq!(top("a\n  b\n    c").unwrap(), vec!["[t(a) [t(b) [t(c)]]]"]);
}

#[test]
fn dedent_back_to_root() {
    assert_eq!(
        top("foo\n    bar\nbaz").unwrap(),
        vec!["[t(foo) [t(bar)]]", "[t(baz)]",],
    );
}

#[test]
fn dedent_multiple_levels() {
    assert_eq!(
        top("a\n  b\n    c\nd").unwrap(),
        vec!["[t(a) [t(b) [t(c)]]]", "[t(d)]",],
    );
}

#[test]
fn child_then_sibling_then_child() {
    assert_eq!(
        top("foo\n    bar\n    baz\n        qux\n    quux\nanother").unwrap(),
        vec![
            "[t(foo) [t(bar)] [t(baz) [t(qux)]] [t(quux)]]",
            "[t(another)]",
        ],
    );
}

#[test]
fn blank_lines_skipped() {
    assert_eq!(
        top("foo\n\n    bar\n\n\nbaz").unwrap(),
        vec!["[t(foo) [t(bar)]]", "[t(baz)]",],
    );
}

#[test]
fn tabs_rejected_a() {
    assert!(top("foo\n\tbar").is_err());
}

#[test]
fn tabs_rejected_b() {
    assert!(top("foo\n  \tbar").is_err());
}

#[test]
fn odd_spaces_rejected_a() {
    assert!(top("foo\n bar").is_err());
}

#[test]
fn odd_spaces_rejected_b() {
    assert!(top("foo\n   bar").is_err());
}

#[test]
fn multi_token_lines_nested() {
    assert_eq!(
        top("if x > 0\n    print pos\n    y = 1\nelse\n    print neg").unwrap(),
        vec![
            "[t(if) t(x) t(>) n(0) [t(print) t(pos)] [t(y) t(=) n(1)]]",
            "[t(else) [t(print) t(neg)]]",
        ],
    );
}

#[test]
fn output_has_no_tabs_or_newlines() {
    assert_eq!(
        top("a\n  b\n    c\n  d\ne").unwrap(),
        vec!["[t(a) [t(b) [t(c)]] [t(d)]]", "[t(e)]",],
    );
}

// --- Brackets and braces suspend indentation ---

#[test]
fn list_literal_open_suspends_indentation() {
    assert_eq!(
        top("LET xs = [\n  1\n  2\n  3\n]").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[n(1) n(2) n(3)]]"]
    );
}

#[test]
fn multiline_list_with_continuation_indent() {
    assert_eq!(
        top("LET xs = [1\n          2\n          3]").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[n(1) n(2) n(3)]]"]
    );
}

#[test]
fn nested_multiline_lists() {
    assert_eq!(
        top("[[1\n  2]\n [3 4]]").unwrap(),
        vec!["[L[L[n(1) n(2)] L[n(3) n(4)]]]"]
    );
}

#[test]
fn balanced_inline_list() {
    assert_eq!(
        top("LET xs = [1 2 3]\nbar").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[n(1) n(2) n(3)]]", "[t(bar)]",],
    );
}

#[test]
fn multiline_dict_literal_continues() {
    assert_eq!(
        top("LET d = {\n  a = 1\n  b = 2\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) R{a = n(1), b = n(2)}]"]
    );
}

#[test]
fn inline_dict_does_not_perturb() {
    assert_eq!(
        top("LET d = {a: 1}\nbar").unwrap(),
        vec!["[t(LET) t(d) t(=) D{t(a): n(1)}]", "[t(bar)]",],
    );
}

#[test]
fn nested_multiline_dict_inside_list() {
    assert_eq!(
        top("[\n  {a: 1\n   b: 2}\n]").unwrap(),
        vec!["[L[D{t(a): n(1), t(b): n(2)}]]"]
    );
}

// --- Trailing-comma continuation ---

#[test]
fn trailing_comma_continues_expression() {
    assert_eq!(top("add 1,\n    2").unwrap(), vec!["[t(add) n(1) n(2)]"]);
}

#[test]
fn trailing_comma_chain_three_lines() {
    assert_eq!(
        top("foo 1,\n    2,\n    3").unwrap(),
        vec!["[t(foo) n(1) n(2) n(3)]"]
    );
}

#[test]
fn trailing_comma_inside_paren_expression() {
    assert_eq!(
        top("UNION Maybe = (some :Number,\n               none :Null)").unwrap(),
        vec!["[t(UNION) T(Maybe) t(=) [t(some) T(Number) t(none) T(Null)]]"]
    );
}

#[test]
fn trailing_comma_through_blank_line() {
    assert_eq!(top("add 1,\n\n    2").unwrap(), vec!["[t(add) n(1) n(2)]"]);
}

#[test]
fn dangling_trailing_comma_at_eof() {
    assert_eq!(top("foo,").unwrap(), vec!["[t(foo)]"]);
}

#[test]
fn no_trailing_comma_keeps_sibling_boundary() {
    assert_eq!(top("foo\nbar").unwrap(), vec!["[t(foo)]", "[t(bar)]",],);
}

// --- Paren continuation across line breaks ---

#[test]
fn open_paren_continues_under_greater_indent() {
    assert_eq!(
        top("PRINT (\n  3.14\n)").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]"]
    );
}

#[test]
fn open_paren_closes_at_deeper_indent() {
    assert_eq!(
        top("PRINT (\n    3.14\n    )").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]"]
    );
}

#[test]
fn open_paren_nests_each_continuation_line() {
    assert_eq!(
        top("FOO (\n  foo\n  bar\n)").unwrap(),
        vec!["[t(FOO) [[t(foo)] [t(bar)]]]"]
    );
}

#[test]
fn nested_multiline_parens_pair_correctly() {
    assert_eq!(
        top("FOO (\n  BAR (\n    x\n  )\n)").unwrap(),
        vec!["[t(FOO) [t(BAR) [t(x)]]]"]
    );
}

#[test]
fn closing_line_nests_as_own_group_a() {
    assert_eq!(
        top("FOO (\n  foo\n  bar\n  baz)").unwrap(),
        vec!["[t(FOO) [[t(foo)] [t(bar)] [t(baz)]]]"]
    );
}

#[test]
fn closing_line_nests_as_own_group_b() {
    assert_eq!(
        top("FOO (\n  (foo)\n  (bar)\n  (baz))").unwrap(),
        vec!["[t(FOO) [[t(foo)] [t(bar)] [t(baz)]]]"]
    );
}

#[test]
fn closing_line_of_several_words() {
    assert_eq!(
        top("FOO (\n  foo\n  bar baz)").unwrap(),
        vec!["[t(FOO) [[t(foo)] [t(bar) t(baz)]]]"]
    );
}

#[test]
fn line_of_nothing_but_closers() {
    assert_eq!(
        top("FOO (\n  BAR (\n    x\n  ))").unwrap(),
        vec!["[t(FOO) [t(BAR) [t(x)]]]"]
    );
}

#[test]
fn closing_line_ends_every_group() {
    assert_eq!(
        top("FOO (\n  BAR (\n    x\n    y))").unwrap(),
        vec!["[t(FOO) [t(BAR) [[t(x)] [t(y)]]]]"]
    );
}

#[test]
fn sigil_led_closing_line() {
    assert_eq!(
        top("FOO (\n  foo\n  #3)").unwrap(),
        vec!["[t(FOO) [[t(foo)] #[n(3)]]]"]
    );
}

#[test]
fn open_paren_same_indent_break_is_error() {
    let error = top("PRINT (\n3.14\n)").unwrap_err();
    assert!(error.contains("unmatched '('"), "got: {error}");
}

#[test]
fn close_paren_below_opener_indent_is_error() {
    let error = top("foo\n  PRINT (\n    3.14\n)").unwrap_err();
    assert!(error.contains("less indented"), "got: {error}");
}

#[test]
fn comma_continuation_overrides_paren_guard() {
    assert_eq!(
        top("PRINT (,\n3.14,\n)").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]"]
    );
}

#[test]
fn balanced_inline_paren() {
    assert_eq!(
        top("PRINT (3.14)\nbar").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]", "[t(bar)]",],
    );
}

// --- Sigil-led lines ---

#[test]
fn quote_sigil_continuation() {
    assert_eq!(
        top("LET x =\n  #3").unwrap(),
        vec!["[t(LET) t(x) t(=) #[n(3)]]"]
    );
}

#[test]
fn eval_sigil_continuation() {
    assert_eq!(top("foo\n  $q").unwrap(), vec!["[t(foo) [t(EVAL) [t(q)]]]"]);
}

#[test]
fn quote_sigil_at_top_level() {
    assert_eq!(top("#3").unwrap(), vec!["[#[n(3)]]"]);
}

#[test]
fn sigil_with_paren_operand() {
    assert_eq!(top("foo\n  #(3)").unwrap(), vec!["[t(foo) #[n(3)]]"]);
}

#[test]
fn sigil_continuation_with_deeper_children() {
    assert_eq!(
        top("foo\n  #bar\n    baz").unwrap(),
        vec!["[t(foo) #[t(bar) [t(baz)]]]"]
    );
}

#[test]
fn comma_continuation_with_bare_sigil() {
    let error = top("add 1,\n  #2").unwrap_err();
    assert!(error.contains("expected '(' after '#'"), "got: {error}");
}

#[test]
fn comma_continuation_with_paren_sigil() {
    assert_eq!(
        top("add 1,\n  #(2)").unwrap(),
        vec!["[t(add) n(1) #[n(2)]]"]
    );
}

#[test]
fn bracket_continuation_with_bare_sigil() {
    let error = top("LET xs = [\n  #3\n]").unwrap_err();
    assert!(error.contains("expected '(' after '#'"), "got: {error}");
}

#[test]
fn bracket_continuation_with_paren_sigils() {
    assert_eq!(
        top("LET xs = [\n  #(3)\n  #(4)\n]").unwrap(),
        vec!["[t(LET) t(xs) t(=) L[#[n(3)] #[n(4)]]]"]
    );
}

#[test]
fn dict_continuation_with_paren_sigils() {
    assert_eq!(
        top("LET d = {\n  x = #(foo)\n  y = #(bar)\n}").unwrap(),
        vec!["[t(LET) t(d) t(=) R{x = #[t(foo)], y = #[t(bar)]}]"]
    );
}

// --- The peel: a body that is exactly one group ---

#[test]
fn r3_a_b() {
    assert_eq!(top("a b").unwrap(), vec!["[t(a) t(b)]"]);
}

#[test]
fn r3_paren_a_b() {
    assert_eq!(top("(a b)").unwrap(), vec!["[t(a) t(b)]"]);
}

#[test]
fn r3_double_paren_a_b() {
    assert_eq!(top("((a b))").unwrap(), vec!["[t(a) t(b)]"]);
}

#[test]
fn r3_print_multiline() {
    assert_eq!(
        top("PRINT (\n  3.14\n)").unwrap(),
        vec!["[t(PRINT) [n(3.14)]]"]
    );
}

#[test]
fn r3_foo_two_body_lines() {
    assert_eq!(
        top("FOO (\n  bar\n  baz\n)").unwrap(),
        vec!["[t(FOO) [[t(bar)] [t(baz)]]]"]
    );
}

#[test]
fn r3_foo_inline_then_body() {
    assert_eq!(top("FOO (a\n  b)").unwrap(), vec!["[t(FOO) [t(a) [t(b)]]]"]);
}

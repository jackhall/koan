use super::{err, ok};
use crate::{ErrorKind, Kind, Node, Span, read};

// --- Lines and indentation ---

#[test]
fn empty_and_blank_input_read_as_nothing() {
    assert_eq!(ok(""), "");
    assert_eq!(ok("   \n\t\n   \n"), "");
}

#[test]
fn each_line_is_a_layout_group() {
    assert_eq!(ok("foo"), "L(foo)");
    assert_eq!(ok("foo bar baz"), "L(foo bar baz)");
    assert_eq!(ok("foo\nbar"), "L(foo) L(bar)");
}

#[test]
fn deeper_lines_nest_and_dedents_close() {
    assert_eq!(ok("a\n  b\n    c\n  d\ne"), "L(a L(b L(c)) L(d)) L(e)");
    assert_eq!(ok("a\n  b\n    c\nd"), "L(a L(b L(c))) L(d)");
}

#[test]
fn a_sibling_may_sit_shallower_than_the_sibling_before_it() {
    // `c` is not inside `b` (it is shallower) and is deeper than `a`, so it is `a`'s child.
    assert_eq!(ok("a\n    b\n  c"), "L(a L(b) L(c))");
}

#[test]
fn the_first_line_sets_no_floor() {
    assert_eq!(ok("  a\nb"), "L(a) L(b)");
}

#[test]
fn blank_lines_are_skipped() {
    assert_eq!(ok("a\n\n  b\n\n\nc"), "L(a L(b)) L(c)");
}

#[test]
fn tab_indentation_is_rejected_on_a_layout_line() {
    let e = err("a\n\tb");
    assert_eq!(e.kind, ErrorKind::TabIndent);
    assert_eq!(e.span, Span { start: 2, end: 3 });
}

#[test]
fn odd_indentation_is_rejected_on_a_layout_line() {
    let e = err("a\n   b");
    assert_eq!(e.kind, ErrorKind::OddIndent { width: 3 });
    assert_eq!(e.span, Span { start: 2, end: 5 });
}

#[test]
fn joined_lines_take_any_indentation() {
    // Inside a bracket, after a trailing comma, and (for the closer) inside a paren body, a
    // line's indentation is free-form.
    assert_eq!(ok("[\n   1\n\t2\n]"), "L([1 2])");
    assert_eq!(ok("add 1,\n   2"), "L(add 1~, 2)");
}

// --- Glue ---

#[test]
fn glue_records_adjacency_and_nothing_else() {
    assert_eq!(ok("#(3)"), "L(#~(3))");
    assert_eq!(ok("# (3)"), "L(# (3))");
    assert_eq!(ok("a.b x?"), "L(a.b x?)");
    assert_eq!(ok("'a'b"), "L('a'~b)");
    assert_eq!(ok("[1]x"), "L([1]~x)");
    assert_eq!(ok("1, 2"), "L(1~, 2)");
    assert_eq!(ok(",,"), "L(,~,)");
}

#[test]
fn the_last_item_of_a_group_is_never_glued() {
    let items = read("(a)b").unwrap();
    let Node::Group { items: line, .. } = &items[0].node else {
        panic!()
    };
    let Node::Group { items: inner, .. } = &line[0].node else {
        panic!()
    };
    assert!(line[0].glued, "the paren is glued to `b`");
    assert!(!inner[0].glued, "`a` has no sibling to be glued to");
}

// --- Strings ---

#[test]
fn strings_keep_their_body_verbatim() {
    assert_eq!(ok("say 'hello' x"), "L(say 'hello' x)");
    assert_eq!(ok("say \"hi 'there'\""), "L(say \"hi 'there'\")");
    assert_eq!(ok(r"say 'hel\'lo'"), r"L(say 'hel\'lo')");
    assert_eq!(ok("''"), "L('')");
}

#[test]
fn a_string_may_span_lines() {
    assert_eq!(ok("a 'x\ny' b"), "L(a 'x\ny' b)");
}

#[test]
fn an_unclosed_string_is_an_error() {
    let e = err("a 'bc");
    assert_eq!(e.kind, ErrorKind::UnclosedString { quote: '\'' });
    assert_eq!(e.span, Span { start: 2, end: 5 });
}

// --- Brackets and braces: flat inside ---

#[test]
fn an_open_bracket_or_brace_suspends_indentation() {
    assert_eq!(ok("LET xs = [\n  1\n  2\n  3\n]"), "L(LET xs = [1 2 3])");
    assert_eq!(
        ok("LET xs = [1\n          2\n          3]"),
        "L(LET xs = [1 2 3])"
    );
    assert_eq!(ok("[[1\n  2]\n [3 4]]"), "L([[1 2] [3 4]])");
    assert_eq!(
        ok("LET d = {\n  a = 1\n  b = 2\n}"),
        "L(LET d = {a = 1 b = 2})"
    );
    assert_eq!(ok("[\n  {a: 1\n   b: 2}\n]"), "L([{a: 1 b: 2}])");
}

#[test]
fn a_paren_inside_a_bracket_is_flat_too() {
    assert_eq!(ok("[(a\nb)]"), "L([(a b)])");
}

#[test]
fn a_balanced_inline_group_does_not_perturb_indentation() {
    assert_eq!(ok("LET xs = [1 2 3]\nbar"), "L(LET xs = [1 2 3]) L(bar)");
    assert_eq!(ok("PRINT (3.14)\nbar"), "L(PRINT (3.14)) L(bar)");
}

// --- Trailing comma ---

#[test]
fn a_trailing_comma_joins_the_next_line_flat() {
    assert_eq!(ok("add 1,\n    2"), "L(add 1~, 2)");
    assert_eq!(ok("foo 1,\n    2,\n    3"), "L(foo 1~, 2~, 3)");
    assert_eq!(ok("add 1,\n\n    2"), "L(add 1~, 2)");
    assert_eq!(ok("foo,\nbar"), "L(foo~, bar)");
}

#[test]
fn a_trailing_comma_at_end_of_input_is_fine() {
    assert_eq!(ok("foo,"), "L(foo~,)");
}

#[test]
fn a_joined_line_does_not_move_the_groups_anchor() {
    // The group is anchored at the first line's indentation, so `bar` is its child.
    assert_eq!(ok("foo 1,\n      2\n  bar"), "L(foo 1~, 2 L(bar))");
}

#[test]
fn a_trailing_comma_inside_a_paren_joins_whatever_the_indentation() {
    assert_eq!(
        ok("UNION Maybe = (some :Number,\n               none :Null)"),
        "L(UNION Maybe = (some :Number~, none :Null))"
    );
    assert_eq!(ok("PRINT (,\n3.14,\n)"), "L(PRINT (, 3.14~,))");
}

// --- Open paren: indentation-sensitive ---

#[test]
fn a_deeper_line_inside_a_paren_is_its_own_layout_group() {
    assert_eq!(ok("PRINT (\n  3.14\n)"), "L(PRINT (L(3.14)))");
    assert_eq!(ok("FOO (\n  A\n  B\n)"), "L(FOO (L(A) L(B)))");
    assert_eq!(ok("FOO (a\n  b)"), "L(FOO (a L(b)))");
}

#[test]
fn body_lines_nest_by_indentation_like_any_others() {
    assert_eq!(ok("FOO (\n  a\n    b\n  c\n)"), "L(FOO (L(a L(b)) L(c)))");
}

#[test]
fn parens_nest_across_lines() {
    assert_eq!(
        ok("FOO (\n  BAR (\n    x\n  )\n)"),
        "L(FOO (L(BAR (L(x)))))"
    );
}

#[test]
fn where_the_closer_sits_is_layout_not_structure() {
    let expected = "L(FOO (L(A) L(B) L(C)))";
    assert_eq!(ok("FOO (\n  A\n  B\n  C\n)"), expected);
    assert_eq!(ok("FOO (\n  A\n  B\n  C)"), expected);
    assert_eq!(ok("FOO (\n  A\n  B\n  C\n    )"), expected);
    assert_eq!(ok("FOO (\n  A\n  B C)"), "L(FOO (L(A) L(B C)))");
}

#[test]
fn a_line_of_nothing_but_closers_closes_each_innermost_paren() {
    assert_eq!(ok("FOO (\n  BAR (\n    x\n  ))"), "L(FOO (L(BAR (L(x)))))");
    assert_eq!(
        ok("FOO (\n  BAR (\n    x\n    y))"),
        "L(FOO (L(BAR (L(x) L(y)))))"
    );
}

#[test]
fn text_after_the_closer_continues_the_opening_line() {
    assert_eq!(ok("FOO (\n  a\n) bar\n  baz"), "L(FOO (L(a)) bar L(baz))");
    assert_eq!(ok("FOO (\n  a\n  c) d"), "L(FOO (L(a) L(c)) d)");
}

#[test]
fn a_closer_may_close_a_paren_with_no_body_from_its_own_line() {
    assert_eq!(ok("FOO (a\n)"), "L(FOO (a))");
}

#[test]
fn a_same_or_shallower_line_before_the_closer_is_a_dangling_paren() {
    let e = err("PRINT (\n3.14\n)");
    assert_eq!(e.kind, ErrorKind::DanglingParen);
    assert_eq!(e.span, Span { start: 6, end: 7 });
    assert!(e.to_string().contains("unmatched '('"));
}

#[test]
fn a_closer_shallower_than_its_opening_line_is_an_error() {
    let e = err("A\n  PRINT (\n    3.14\n)");
    assert_eq!(
        e.kind,
        ErrorKind::CloserDedented {
            opener: Span { start: 10, end: 11 }
        }
    );
    assert!(e.to_string().contains("less indented"));
}

#[test]
fn a_comma_overrides_the_paren_indentation_guard() {
    assert_eq!(ok("PRINT (a,\nb\n)"), "L(PRINT (a~, b))");
}

// --- Sigil-led lines are ordinary lines here ---

#[test]
fn a_sigil_led_line_is_just_a_line_whose_first_atom_starts_with_the_sigil() {
    // The layer above reads `L(#bar L(baz))` as a quote of `bar (baz)`.
    assert_eq!(ok("foo\n  #bar\n    baz"), "L(foo L(#bar L(baz)))");
    assert_eq!(ok("#3"), "L(#3)");
    assert_eq!(ok("LET x =\n  #(3)"), "L(LET x = L(#~(3)))");
}

// --- Structural errors ---

#[test]
fn an_unclosed_group_points_at_its_opener() {
    let e = err("a (b\n  c");
    assert_eq!(e.kind, ErrorKind::UnclosedGroup { kind: Kind::Paren });
    assert_eq!(e.span, Span { start: 2, end: 3 });
    let e = err("[1 2");
    assert_eq!(
        e.kind,
        ErrorKind::UnclosedGroup {
            kind: Kind::Bracket
        }
    );
}

#[test]
fn a_closer_with_nothing_open_is_an_error() {
    let e = err("a)");
    assert_eq!(e.kind, ErrorKind::UnexpectedCloser { kind: Kind::Paren });
    assert_eq!(e.span, Span { start: 1, end: 2 });
    let e = err("a\n]");
    assert_eq!(
        e.kind,
        ErrorKind::UnexpectedCloser {
            kind: Kind::Bracket
        }
    );
}

#[test]
fn a_closer_of_the_wrong_family_names_both_ends() {
    let e = err("[a)");
    assert_eq!(
        e.kind,
        ErrorKind::MismatchedCloser {
            opened: Kind::Bracket,
            opener: Span { start: 0, end: 1 },
            found: Kind::Paren,
        }
    );
    assert_eq!(e.span, Span { start: 2, end: 3 });
    assert_eq!(
        err("(a]").kind,
        ErrorKind::MismatchedCloser {
            opened: Kind::Paren,
            opener: Span { start: 0, end: 1 },
            found: Kind::Bracket,
        }
    );
}

// --- Spans ---

#[test]
fn spans_cover_delimiters_and_layout_groups_cover_their_items() {
    let source = "foo (bar 'x')\n  baz";
    let items = read(source).unwrap();
    assert_eq!(items[0].span, Span { start: 0, end: 19 });
    let Node::Group { items: line, .. } = &items[0].node else {
        panic!()
    };
    assert_eq!(line[0].span.slice(source), "foo");
    assert_eq!(line[1].span.slice(source), "(bar 'x')");
    let Node::Group { items: paren, .. } = &line[1].node else {
        panic!()
    };
    assert_eq!(paren[1].span.slice(source), "'x'");
    assert_eq!(line[2].span.slice(source), "baz");
}

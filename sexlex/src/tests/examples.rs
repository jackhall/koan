use super::{err, ok};
use crate::{ErrorKind, Kind, MAX_DEPTH, Node, Span, read, read_group_at, render};

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

// --- Comments ---

#[test]
fn a_comment_runs_to_the_end_of_its_line() {
    assert_eq!(ok("a b ; c d"), "L(a b)");
    assert_eq!(ok("a ; c\nb"), "L(a) L(b)");
    // A `;` ends an atom, as whitespace does.
    assert_eq!(ok("a;b"), "L(a)");
    assert_eq!(ok("x;"), "L(x)");
}

#[test]
fn a_comment_holds_unmatched_brackets_and_quotes() {
    assert_eq!(ok("[a ; ) ] ' \"\n b]"), "L([a b])");
    assert_eq!(ok("(a ; (\n)"), "L((a))");
}

#[test]
fn a_comment_only_line_is_no_layout_line() {
    // It neither closes the block above it nor opens one.
    assert_eq!(ok("a\n; note\n  b"), "L(a L(b))");
    assert_eq!(ok("a\n  b\n    ; note\n  c"), "L(a L(b) L(c))");
    // Its indentation is never judged, as a blank line's is not.
    assert_eq!(ok("a\n   ; odd\n\t; tab\n  b"), "L(a L(b))");
    assert_eq!(ok("; only a comment"), "");
}

#[test]
fn a_comment_keeps_a_trailing_comma_joining() {
    assert_eq!(ok("add 1, ; more\n   2"), "L(add 1~, 2)");
    assert_eq!(ok("add 1,\n; note\n   2"), "L(add 1~, 2)");
}

#[test]
fn a_semicolon_in_a_string_is_text() {
    assert_eq!(ok("'a;b' \"c ; d\""), "L('a;b' \"c ; d\")");
    // A line of a string's body that starts with `;` is still the string's.
    assert_eq!(ok("'a\n; b'"), "L('a\n; b')");
}

#[test]
fn a_comment_falls_outside_every_span() {
    let items = read("a ; note\n").unwrap();
    assert_eq!(items[0].span, Span { start: 0, end: 1 });
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

#[test]
fn groups_nest_up_to_the_limit_and_no_deeper() {
    // A debug build's reader frames outgrow the default test thread well before the limit; the
    // language on top picks the stack it reads on.
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(groups_nest_to_the_limit)
        .expect("spawn")
        .join()
        .expect("reads without overflow");
}

fn groups_nest_to_the_limit() {
    // The line's layout group is depth 1, so `MAX_DEPTH - 1` parens reach the limit exactly.
    let nested = |parens: usize| format!("{}a{}", "(".repeat(parens), ")".repeat(parens));
    assert!(read(&nested(MAX_DEPTH - 1)).is_ok());
    let source = nested(MAX_DEPTH);
    let e = err(&source);
    assert_eq!(e.kind, ErrorKind::TooDeep { limit: MAX_DEPTH });
    assert_eq!(e.span.start as usize, MAX_DEPTH - 1);
    assert!(e.to_string().contains(&MAX_DEPTH.to_string()));
    // Indentation nests layout groups, and they count too.
    let lines: String = (0..MAX_DEPTH + 1)
        .map(|level| format!("{}a\n", "  ".repeat(level)))
        .collect();
    assert_eq!(err(&lines).kind, ErrorKind::TooDeep { limit: MAX_DEPTH });
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

// --- Reading one group ---

#[test]
fn read_group_at_indexes_the_larger_source() {
    let brace = read_group_at("{a b}", 10).unwrap();
    assert_eq!(brace.span, Span { start: 10, end: 15 });
    let Node::Group {
        kind: Kind::Brace,
        items,
    } = &brace.node
    else {
        panic!("a brace")
    };
    let spans: Vec<Span> = items.iter().map(|item| item.span).collect();
    assert_eq!(
        spans,
        [Span { start: 11, end: 12 }, Span { start: 13, end: 14 }]
    );
    let error = read_group_at("{a", 10).unwrap_err();
    assert_eq!(error.span, Span { start: 10, end: 11 });
}

#[test]
fn read_group_at_reads_nothing_past_the_balancing_closer() {
    // Not even an unclosed quote.
    let group = read_group_at("{a {b}} c 'd", 0).unwrap();
    assert_eq!(group.span, Span { start: 0, end: 7 });
    assert!(!group.glued);
    // Every family counts toward the balance, so a stray closer is a mismatch.
    assert!(matches!(
        read_group_at("{a ) b}", 0).unwrap_err().kind,
        ErrorKind::MismatchedCloser { .. }
    ));
    // A mismatch ends the read too, before an unclosed quote past it.
    let error = read_group_at("{(a} 'b", 0).unwrap_err();
    assert!(matches!(error.kind, ErrorKind::MismatchedCloser { .. }));
    assert_eq!(error.span, Span { start: 3, end: 4 });
    // A paren keeps its layout regime.
    let paren = read_group_at("(a\n  b\n)", 0).unwrap();
    assert_eq!(render(std::slice::from_ref(&paren)), "(a L(b))");
}

#[test]
fn a_comment_in_a_group_read_alone_runs_to_its_line_end() {
    assert_eq!(
        read_group_at("{a ; b}", 0).unwrap_err().kind,
        ErrorKind::UnclosedGroup { kind: Kind::Brace }
    );
    let brace = read_group_at("{a ; b\n}", 0).unwrap();
    assert_eq!(brace.span, Span { start: 0, end: 8 });
}

#[test]
#[should_panic(expected = "opens with a bracket")]
fn read_group_at_wants_an_opener() {
    let _ = read_group_at("a", 0);
}

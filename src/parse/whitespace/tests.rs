use super::collapse_whitespace as collapse_bytes;
use crate::parse::quotes::JUMP_MARK;

/// `&str` → `String` shim that strips JUMP markers so the paren-shape assertions stay
/// readable. Marker-aware tests at the bottom of the module use the raw byte stream.
fn collapse_whitespace(input: &str) -> Result<String, String> {
    collapse_bytes(input.as_bytes())
        .map(|v| String::from_utf8(strip_jumps(&v)).expect("UTF-8 in test"))
        .map_err(|e| e.to_string())
}

/// Strip `JUMP_MARK <digits> JUMP_MARK` runs; LITERAL markers pass through untouched.
fn strip_jumps(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == JUMP_MARK {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == JUMP_MARK && j > i + 1 {
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[test]
fn empty_input() {
    assert_eq!(collapse_whitespace("").unwrap(), "");
}

#[test]
fn only_whitespace() {
    assert_eq!(collapse_whitespace("   \n\t\n   \n").unwrap(), "");
}

#[test]
fn single_line() {
    assert_eq!(collapse_whitespace("foo").unwrap(), "(foo)");
}

#[test]
fn single_line_multiple_tokens() {
    assert_eq!(collapse_whitespace("foo bar baz").unwrap(), "(foo bar baz)");
}

#[test]
fn sibling_lines() {
    assert_eq!(collapse_whitespace("foo\nbar").unwrap(), "(foo) (bar)");
}

#[test]
fn parent_with_child() {
    assert_eq!(collapse_whitespace("foo\n    bar").unwrap(), "(foo (bar))");
}

#[test]
fn parent_with_two_children() {
    assert_eq!(
        collapse_whitespace("foo\n    bar\n    baz").unwrap(),
        "(foo (bar) (baz))"
    );
}

#[test]
fn nested_three_deep() {
    assert_eq!(
        collapse_whitespace("a\n  b\n    c").unwrap(),
        "(a (b (c)))"
    );
}

#[test]
fn dedent_back_to_root() {
    assert_eq!(
        collapse_whitespace("foo\n    bar\nbaz").unwrap(),
        "(foo (bar)) (baz)"
    );
}

#[test]
fn dedent_multiple_levels() {
    assert_eq!(
        collapse_whitespace("a\n  b\n    c\nd").unwrap(),
        "(a (b (c))) (d)"
    );
}

#[test]
fn child_then_sibling_then_child() {
    assert_eq!(
        collapse_whitespace("foo\n    bar\n    baz\n        qux\n    quux\nanother").unwrap(),
        "(foo (bar) (baz (qux)) (quux)) (another)"
    );
}

#[test]
fn blank_lines_skipped() {
    assert_eq!(
        collapse_whitespace("foo\n\n    bar\n\n\nbaz").unwrap(),
        "(foo (bar)) (baz)"
    );
}

#[test]
fn tabs_rejected() {
    assert!(collapse_whitespace("foo\n\tbar").is_err());
    assert!(collapse_whitespace("foo\n  \tbar").is_err());
}

#[test]
fn odd_spaces_rejected() {
    assert!(collapse_whitespace("foo\n bar").is_err());
    assert!(collapse_whitespace("foo\n   bar").is_err());
}

#[test]
fn multi_token_lines_nested() {
    assert_eq!(
        collapse_whitespace("if x > 0\n    print pos\n    y = 1\nelse\n    print neg").unwrap(),
        "(if x > 0 (print pos) (y = 1)) (else (print neg))"
    );
}

#[test]
fn output_has_no_tabs_or_newlines() {
    let out = collapse_whitespace("a\n  b\n    c\n  d\ne").unwrap();
    assert!(!out.contains('\n'));
    assert!(!out.contains('\t'));
}

#[test]
fn list_literal_open_suspends_indentation_handling() {
    assert_eq!(
        collapse_whitespace("LET xs = [\n  1\n  2\n  3\n]").unwrap(),
        "(LET xs = [ 1 2 3 ])",
    );
}

#[test]
fn multiline_list_with_continuation_indent() {
    assert_eq!(
        collapse_whitespace("LET xs = [1\n          2\n          3]").unwrap(),
        "(LET xs = [1 2 3])",
    );
}

#[test]
fn nested_multiline_lists() {
    assert_eq!(
        collapse_whitespace("[[1\n  2]\n [3 4]]").unwrap(),
        "([[1 2] [3 4]])",
    );
}

#[test]
fn balanced_inline_list_does_not_perturb_indentation() {
    assert_eq!(
        collapse_whitespace("LET xs = [1 2 3]\nbar").unwrap(),
        "(LET xs = [1 2 3]) (bar)",
    );
}

#[test]
fn multiline_dict_literal_continues() {
    assert_eq!(
        collapse_whitespace("LET d = {\n  a = 1\n  b = 2\n}").unwrap(),
        "(LET d = { a = 1 b = 2 })",
    );
}

#[test]
fn inline_dict_does_not_perturb_indentation() {
    assert_eq!(
        collapse_whitespace("LET d = {a: 1}\nbar").unwrap(),
        "(LET d = {a: 1}) (bar)",
    );
}

#[test]
fn nested_multiline_dict_inside_list() {
    assert_eq!(
        collapse_whitespace("[\n  {a: 1\n   b: 2}\n]").unwrap(),
        "([ {a: 1 b: 2} ])",
    );
}

// --- Trailing-comma line continuation ---

#[test]
fn trailing_comma_continues_expression() {
    assert_eq!(
        collapse_whitespace("add 1,\n    2").unwrap(),
        "(add 1, 2)",
    );
}

#[test]
fn trailing_comma_chain_across_three_lines() {
    assert_eq!(
        collapse_whitespace("foo 1,\n    2,\n    3").unwrap(),
        "(foo 1, 2, 3)",
    );
}

#[test]
fn trailing_comma_inside_paren_expression() {
    // Motivating UNION shape.
    assert_eq!(
        collapse_whitespace("UNION Maybe = (some :Number,\n               none :Null)")
            .unwrap(),
        "(UNION Maybe = (some :Number, none :Null))",
    );
}

#[test]
fn trailing_comma_continuation_through_blank_line() {
    assert_eq!(
        collapse_whitespace("add 1,\n\n    2").unwrap(),
        "(add 1, 2)",
    );
}

#[test]
fn dangling_trailing_comma_at_eof() {
    assert_eq!(collapse_whitespace("foo,").unwrap(), "(foo,)");
}

#[test]
fn no_trailing_comma_keeps_sibling_boundary() {
    assert_eq!(collapse_whitespace("foo\nbar").unwrap(), "(foo) (bar)");
}

// --- Paren continuation across line breaks ---

#[test]
fn open_paren_continues_under_greater_indent() {
    assert_eq!(
        collapse_whitespace("PRINT (\n  3.14\n)").unwrap(),
        "(PRINT ( (3.14 )))",
    );
}

#[test]
fn open_paren_closes_at_deeper_indent() {
    assert_eq!(
        collapse_whitespace("PRINT (\n    3.14\n    )").unwrap(),
        "(PRINT ( (3.14 )))",
    );
}

#[test]
fn open_paren_nests_each_continuation_line() {
    // Nest-per-line, not flattened: body is `(A) (B)`.
    assert_eq!(
        collapse_whitespace("FOO (\n  A\n  B\n)").unwrap(),
        "(FOO ( (A) (B )))",
    );
}

#[test]
fn nested_multiline_parens_pair_correctly() {
    assert_eq!(
        collapse_whitespace("FOO (\n  BAR (\n    x\n  )\n)").unwrap(),
        "(FOO ( (BAR ( (x ) ))))",
    );
}

#[test]
fn open_paren_same_indent_break_is_error() {
    let err = collapse_whitespace("PRINT (\n3.14\n)").unwrap_err();
    assert!(err.contains("unmatched '('"), "got: {err}");
}

#[test]
fn close_paren_below_opener_indent_is_error() {
    let err = collapse_whitespace("A\n  PRINT (\n    3.14\n)").unwrap_err();
    assert!(err.contains("less indented"), "got: {err}");
}

#[test]
fn comma_continuation_overrides_paren_indent_guard() {
    // Comma overrides the same-indent dangling-`(` guard; comma lines join flat.
    assert_eq!(
        collapse_whitespace("PRINT (,\n3.14,\n)").unwrap(),
        "(PRINT (, 3.14, ))",
    );
}

#[test]
fn balanced_inline_paren_does_not_perturb_indentation() {
    assert_eq!(
        collapse_whitespace("PRINT (3.14)\nbar").unwrap(),
        "(PRINT (3.14)) (bar)",
    );
}

// --- Sigil-led continuation lines ---

#[test]
fn quote_sigil_continuation_wraps_outside_paren() {
    // `#3` must collapse to `#(3)`, not `(#3)`, to satisfy the sigil-adjacency rule.
    assert_eq!(
        collapse_whitespace("LET x =\n  #3").unwrap(),
        "(LET x = #(3))",
    );
}

#[test]
fn eval_sigil_continuation_wraps_outside_paren() {
    assert_eq!(
        collapse_whitespace("foo\n  $q").unwrap(),
        "(foo $(q))",
    );
}

#[test]
fn quote_sigil_at_top_level_wraps_outside_paren() {
    assert_eq!(collapse_whitespace("#3").unwrap(), "#(3)");
}

#[test]
fn sigil_with_paren_operand_still_legal() {
    // Double wrapping is fine: `peel_redundant` collapses it downstream.
    assert_eq!(
        collapse_whitespace("foo\n  #(3)").unwrap(),
        "(foo #((3)))",
    );
}

#[test]
fn sigil_continuation_with_deeper_children() {
    assert_eq!(
        collapse_whitespace("foo\n  #bar\n    baz").unwrap(),
        "(foo #(bar (baz)))",
    );
}

// --- Sigils on comma- and bracket-continuation lines (no wrap-operand fix) ---
//
// The wrap-outside-paren rewrite only runs on the indent-driven path. Flat-continuation
// lines append verbatim, so a bare `#sym` stays bare and `build_tree` rejects it. These
// tests lock that contract in — users must spell `#(sym)` inside comma/list/dict
// continuations.

#[test]
fn comma_continuation_with_bare_sigil_stays_bare() {
    assert_eq!(
        collapse_whitespace("add 1,\n  #2").unwrap(),
        "(add 1, #2)",
    );
}

#[test]
fn comma_continuation_with_paren_sigil_passes_through() {
    assert_eq!(
        collapse_whitespace("add 1,\n  #(2)").unwrap(),
        "(add 1, #(2))",
    );
}

#[test]
fn bracket_continuation_with_bare_sigil_stays_bare() {
    assert_eq!(
        collapse_whitespace("LET xs = [\n  #3\n]").unwrap(),
        "(LET xs = [ #3 ])",
    );
}

#[test]
fn bracket_continuation_with_paren_sigils_passes_through() {
    assert_eq!(
        collapse_whitespace("LET xs = [\n  #(3)\n  #(4)\n]").unwrap(),
        "(LET xs = [ #(3) #(4) ])",
    );
}

#[test]
fn dict_continuation_with_paren_sigils_passes_through() {
    // Motivating dict-as-struct shape: each value is a `#(...)` QUOTE.
    assert_eq!(
        collapse_whitespace("LET d = {\n  x = #(foo)\n  y = #(bar)\n}").unwrap(),
        "(LET d = { x = #(foo) y = #(bar) })",
    );
}

// --- JUMP marker placement ---
//
// Raw-byte assertions, including the `JUMP_MARK <offset> JUMP_MARK` cursor anchors. The
// downstream span recovery reads these payloads, so the offsets are load-bearing.

use crate::parse::quotes::{LEN_SEP, LITERAL_MARK};

fn jmp(offset: u32) -> Vec<u8> {
    let mut v = vec![JUMP_MARK];
    v.extend_from_slice(offset.to_string().as_bytes());
    v.push(JUMP_MARK);
    v
}

/// LITERAL marker matching `mask_quotes`'s form, for synthetic post-mask inputs.
fn lit(idx: usize, len: usize) -> Vec<u8> {
    let mut v = vec![LITERAL_MARK];
    v.extend_from_slice(idx.to_string().as_bytes());
    v.push(LEN_SEP);
    v.extend_from_slice(len.to_string().as_bytes());
    v
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

fn raw(input: &str) -> Vec<u8> {
    collapse_bytes(input.as_bytes()).expect("collapse")
}

fn raw_bytes(input: &[u8]) -> Vec<u8> {
    collapse_bytes(input).expect("collapse")
}

#[test]
fn single_line_anchors_line_open_and_close() {
    // `foo` (3 bytes, no trailing newline). Cursor: 0 at line open, 3 one past `o`.
    assert_eq!(raw("foo"), cat(&[&jmp(0), b"(foo", &jmp(3), b")"]));
}

#[test]
fn sibling_lines_anchor_each_paren() {
    // `foo\nbar` (7 bytes). Line 1 spans 0..3, line 2 spans 4..7. Each `(` snaps
    // to the next content byte; each `)` snaps to one past the previous content.
    assert_eq!(
        raw("foo\nbar"),
        cat(&[
            &jmp(0), b"(foo",
            &jmp(3), b") ",
            &jmp(4), b"(bar",
            &jmp(7), b")",
        ]),
    );
}

#[test]
fn nested_block_anchors_per_frame() {
    // `a\n  b` (5 bytes). Line 2 sits below line 1; closing chars at EOF snap to 5.
    assert_eq!(
        raw("a\n  b"),
        cat(&[
            &jmp(0), b"(a ",
            &jmp(4), b"(b",
            &jmp(5), b")",
            &jmp(5), b")",
        ]),
    );
}

#[test]
fn dedent_then_sibling_anchors_correctly() {
    // `foo\n    bar\nbaz` (15 bytes). Indent on line 2 nests it; line 3 is a sibling
    // of line 1. Two `)`s fire on line-3 dedent, both anchored to 11 (one past `r`).
    assert_eq!(
        raw("foo\n    bar\nbaz"),
        cat(&[
            &jmp(0), b"(foo ",
            &jmp(8), b"(bar",
            &jmp(11), b")",
            &jmp(11), b") ",
            &jmp(12), b"(baz",
            &jmp(15), b")",
        ]),
    );
}

#[test]
fn continuation_join_space_carries_anchor() {
    // `add 1,\n  2` (10 bytes). Trailing comma carries the second line into the same
    // frame; the synthetic joining space is preceded by a JUMP at orig of `2`.
    assert_eq!(
        raw("add 1,\n  2"),
        cat(&[
            &jmp(0), b"(add 1,",
            &jmp(9), b" 2",
            &jmp(10), b")",
        ]),
    );
}

#[test]
fn sigil_led_top_level_emits_two_jumps_around_sigil() {
    // `#3` (2 bytes). The `#` is real content (JUMP 0 before it); the synthetic `(`
    // gets its own JUMP at offset 1 (orig of `3`).
    assert_eq!(
        raw("#3"),
        cat(&[
            &jmp(0), b"#",
            &jmp(1), b"(3",
            &jmp(2), b")",
        ]),
    );
}

#[test]
fn sigil_led_continuation_anchors_at_real_sigil_offset() {
    // `foo\n  #3` (8 bytes). The continuation `#` sits at orig 6; the rest of the
    // sigil-wrapped paren spans orig 7..8.
    assert_eq!(
        raw("foo\n  #3"),
        cat(&[
            &jmp(0), b"(foo ",
            &jmp(6), b"#",
            &jmp(7), b"(3",
            &jmp(8), b")",
            &jmp(8), b")",
        ]),
    );
}

#[test]
fn literal_passthrough_keeps_mask_jump_then_emits_close_jump() {
    // Synthetic post-mask input for `'hello'` (orig 7 bytes). `mask_quotes` would emit
    // the opening quote + LITERAL marker + closing quote + JUMP-past-close; we feed
    // that shape to `collapse_whitespace` directly to confirm it passes the literal
    // sequence through verbatim and lands the closing `)` JUMP at orig 7.
    let masked = cat(&[b"'", &lit(0, 5), b"'", &jmp(7)]);
    let expected = cat(&[
        &jmp(0), b"('",
        &lit(0, 5),
        b"'",
        &jmp(7),
        &jmp(7), b")",
    ]);
    assert_eq!(raw_bytes(&masked), expected);
}

#[test]
fn blank_lines_advance_cursor_for_following_anchors() {
    // `foo\n\nbar` (8 bytes). Blank line 2 has length 0 + the `\n` between it and
    // line 3; the JUMP before line 3's `(` must reflect orig 5 (start of `bar`).
    assert_eq!(
        raw("foo\n\nbar"),
        cat(&[
            &jmp(0), b"(foo",
            &jmp(3), b") ",
            &jmp(5), b"(bar",
            &jmp(8), b")",
        ]),
    );
}

#[test]
fn list_continuation_anchors_each_joined_line() {
    // `[1\n 2]` (6 bytes). Open `[` carries delim_depth, so line 2 enters via the
    // continuation branch; each joining space carries an anchor at the line's
    // content start.
    assert_eq!(
        raw("[1\n 2]"),
        cat(&[
            &jmp(0), b"([1",
            &jmp(4), b" 2]",
            &jmp(6), b")",
        ]),
    );
}

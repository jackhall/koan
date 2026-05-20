use super::collapse_whitespace as collapse_bytes;
use crate::parse::quotes::JUMP_MARK;

/// Test shim: keep the readable `&str` → `String` ergonomics from the pre-Phase-2 API.
/// Phase 3 added JUMP markers around every synthetic char; the existing assertions check
/// the high-level paren shape, so we strip the markers before comparing. Marker-aware
/// tests at the bottom of this module assert against the raw byte stream instead.
fn collapse_whitespace(input: &str) -> Result<String, String> {
    collapse_bytes(input.as_bytes())
        .map(|v| String::from_utf8(strip_jumps(&v)).expect("UTF-8 in test"))
        .map_err(|e| e.to_string())
}

/// Strip `JUMP_MARK <digits> JUMP_MARK` runs from a masked byte stream so textual
/// assertions don't have to encode the cursor anchors. LITERAL markers pass through
/// untouched — they survive into `build_tree` regardless of phase.
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
    // The `[` on line 1 stays open across lines 2–4, so those lines append to the list
    // span instead of becoming nested paren groups. The closing `]` brings depth back to 0.
    assert_eq!(
        collapse_whitespace("LET xs = [\n  1\n  2\n  3\n]").unwrap(),
        "(LET xs = [ 1 2 3 ])",
    );
}

#[test]
fn multiline_list_with_continuation_indent() {
    // The `[1` opens at the end of line 1; lines 2 and 3 sit under it as continuation,
    // not as deeper-indent children. Final `]` closes the span.
    assert_eq!(
        collapse_whitespace("LET xs = [1\n          2\n          3]").unwrap(),
        "(LET xs = [1 2 3])",
    );
}

#[test]
fn nested_multiline_lists() {
    // Inner `]` brings depth from 2 to 1 mid-line; outer `]` closes back to 0 on the
    // last line.
    assert_eq!(
        collapse_whitespace("[[1\n  2]\n [3 4]]").unwrap(),
        "([[1 2] [3 4]])",
    );
}

#[test]
fn balanced_inline_list_does_not_perturb_indentation() {
    // `[1 2 3]` balances within its line, so depth stays at 0 and the indentation pass
    // continues normally — the next line becomes a sibling group as it would without
    // brackets at all.
    assert_eq!(
        collapse_whitespace("LET xs = [1 2 3]\nbar").unwrap(),
        "(LET xs = [1 2 3]) (bar)",
    );
}

#[test]
fn multiline_dict_literal_continues() {
    // Same continuation rule as lists: `{` opens, lines append, `}` closes.
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
    // List opens on line 1, dict opens inside on line 2; both close on the last line.
    assert_eq!(
        collapse_whitespace("[\n  {a: 1\n   b: 2}\n]").unwrap(),
        "([ {a: 1 b: 2} ])",
    );
}

// --- Trailing-comma line continuation ---

#[test]
fn trailing_comma_continues_expression() {
    // The `,` at end of line 1 suspends indentation handling; line 2 appends to the open
    // group instead of becoming a child block.
    assert_eq!(
        collapse_whitespace("add 1,\n    2").unwrap(),
        "(add 1, 2)",
    );
}

#[test]
fn trailing_comma_chain_across_three_lines() {
    // Continuation persists as long as each line keeps ending in `,`.
    assert_eq!(
        collapse_whitespace("foo 1,\n    2,\n    3").unwrap(),
        "(foo 1, 2, 3)",
    );
}

#[test]
fn trailing_comma_inside_paren_expression() {
    // The motivating UNION shape: open paren on line 1, comma signals continuation,
    // close paren on line 2.
    assert_eq!(
        collapse_whitespace("UNION Maybe = (some :Number,\n               none :Null)")
            .unwrap(),
        "(UNION Maybe = (some :Number, none :Null))",
    );
}

#[test]
fn trailing_comma_continuation_through_blank_line() {
    // Blank lines are skipped before the continuation check, so they don't break a
    // comma chain — same shape Python uses inside bracket continuations.
    assert_eq!(
        collapse_whitespace("add 1,\n\n    2").unwrap(),
        "(add 1, 2)",
    );
}

#[test]
fn dangling_trailing_comma_at_eof() {
    // No following line to consume the continuation; the `,` rides through unchanged.
    // `build_tree` drops it as a no-op once it sees an expression-frame `,`.
    assert_eq!(collapse_whitespace("foo,").unwrap(), "(foo,)");
}

#[test]
fn no_trailing_comma_keeps_sibling_boundary() {
    // Guard: lines that don't end in `,` still produce sibling groups.
    assert_eq!(collapse_whitespace("foo\nbar").unwrap(), "(foo) (bar)");
}

// --- Paren continuation across line breaks ---

#[test]
fn open_paren_continues_under_greater_indent() {
    // `PRINT (` leaves a paren open; the deeper `3.14` line nests inside it as its own
    // group, and the `)` at the opening indent closes the literal paren. Each continuation
    // line is wrapped (nest-per-line), so the body is `((3.14))`.
    assert_eq!(
        collapse_whitespace("PRINT (\n  3.14\n)").unwrap(),
        "(PRINT ( (3.14 )))",
    );
}

#[test]
fn open_paren_closes_at_deeper_indent() {
    // The matching `)` may itself sit on a deeper-indent continuation line (>= the opener),
    // and still closes the group; it never triggered an expression break.
    assert_eq!(
        collapse_whitespace("PRINT (\n    3.14\n    )").unwrap(),
        "(PRINT ( (3.14 )))",
    );
}

#[test]
fn open_paren_nests_each_continuation_line() {
    // Two deeper lines under an open paren each wrap as their own nested group, so the
    // paren body is `(A) (B)` — nest-per-line, not a flattened argument list.
    assert_eq!(
        collapse_whitespace("FOO (\n  A\n  B\n)").unwrap(),
        "(FOO ( (A) (B )))",
    );
}

#[test]
fn nested_multiline_parens_pair_correctly() {
    // An inner `(` opened on a deeper line closes at its own indent before the outer `)`
    // closes at the opener's. The anchor stack keeps each paren matched to its own opener.
    assert_eq!(
        collapse_whitespace("FOO (\n  BAR (\n    x\n  )\n)").unwrap(),
        "(FOO ( (BAR ( (x ) ))))",
    );
}

#[test]
fn open_paren_same_indent_break_is_error() {
    // The dangling-`(` case: the `(` opens at indent 0, then `3.14` breaks at the same
    // indentation without closing it. A clear parse error, not a downstream dispatch
    // failure on an empty `()` group.
    let err = collapse_whitespace("PRINT (\n3.14\n)").unwrap_err();
    assert!(err.contains("unmatched '('"), "got: {err}");
}

#[test]
fn close_paren_below_opener_indent_is_error() {
    // The opener sits at indent 2; the `)` dedents to indent 0, below its opener. Closing
    // a paren shallower than where it opened is rejected (same-or-greater close rule).
    let err = collapse_whitespace("A\n  PRINT (\n    3.14\n)").unwrap_err();
    assert!(err.contains("less indented"), "got: {err}");
}

#[test]
fn comma_continuation_overrides_paren_indent_guard() {
    // A trailing comma is an explicit continuation, so a same-indent next line is allowed
    // even with the paren still open (the motivating multi-line UNION shape). Comma lines
    // join flat rather than nesting.
    assert_eq!(
        collapse_whitespace("PRINT (,\n3.14,\n)").unwrap(),
        "(PRINT (, 3.14, ))",
    );
}

#[test]
fn balanced_inline_paren_does_not_perturb_indentation() {
    // A line whose parens balance within it (`PRINT (3.14)`) leaves no paren open, so the
    // following line becomes a sibling group as usual.
    assert_eq!(
        collapse_whitespace("PRINT (3.14)\nbar").unwrap(),
        "(PRINT (3.14)) (bar)",
    );
}

// --- Sigil-led continuation lines ---

#[test]
fn quote_sigil_continuation_wraps_outside_paren() {
    // `#3` on a continuation line must collapse to `#(3)`, not `(#3)` — the latter
    // violates `expression_tree`'s sigil-adjacency rule (sigil glued to a non-paren).
    assert_eq!(
        collapse_whitespace("LET x =\n  #3").unwrap(),
        "(LET x = #(3))",
    );
}

#[test]
fn eval_sigil_continuation_wraps_outside_paren() {
    // Symmetric case for `$`: `$q` collapses to `$(q)` so the parser sees the sigil
    // immediately followed by `(`.
    assert_eq!(
        collapse_whitespace("foo\n  $q").unwrap(),
        "(foo $(q))",
    );
}

#[test]
fn quote_sigil_at_top_level_wraps_outside_paren() {
    // The same rule applies even when the sigil-led line is itself the root of the
    // collapse (no parent expression). `#3` collapses to `#(3)`.
    assert_eq!(collapse_whitespace("#3").unwrap(), "#(3)");
}

#[test]
fn sigil_with_paren_operand_still_legal() {
    // `#(3)` written on a continuation line collapses to `#((3))`. The double wrapping
    // is harmless: `peel_redundant` in `build_tree` strips extra single-`Expression`
    // wrappers downstream.
    assert_eq!(
        collapse_whitespace("foo\n  #(3)").unwrap(),
        "(foo #((3)))",
    );
}

#[test]
fn sigil_continuation_with_deeper_children() {
    // Deeper-indented children of a sigil-led line live inside the sigil's group, so
    // the sigil applies to the whole sub-block.
    assert_eq!(
        collapse_whitespace("foo\n  #bar\n    baz").unwrap(),
        "(foo #(bar (baz)))",
    );
}

// --- Sigils on comma- and bracket-continuation lines (no wrap-operand fix) ---
//
// The wrap-outside-paren rewrite only runs on the indent-driven path. Lines consumed by
// the comma-continuation or open-bracket/dict continuation path are appended verbatim,
// so a bare `#sym` on those lines stays bare and reaches `build_tree` to be rejected by
// the sigil-adjacency rule. These tests lock that contract in: the user gets a clear
// parse error and must spell out `#(sym)` explicitly when continuing into a list/dict
// literal or trailing-comma chain.

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
    // The motivating dict-as-struct shape from the roadmap: each value is a `#(...)`
    // QUOTE that the struct constructor will dispatch on later.
    assert_eq!(
        collapse_whitespace("LET d = {\n  x = #(foo)\n  y = #(bar)\n}").unwrap(),
        "(LET d = { x = #(foo) y = #(bar) })",
    );
}

// --- Phase 3: JUMP marker placement ---
//
// These tests assert against the *raw* byte stream emitted by `collapse_whitespace`,
// including the `JUMP_MARK <offset> JUMP_MARK` cursor anchors that the pass inserts
// around every synthetic char. `build_tree` consumes-and-ignores the payloads today
// (Phase 2 behaviour, unchanged in Phase 3) but Phase 4 will read them to populate
// `KExpression::span`, so locking the offsets in now catches regressions early.

use crate::parse::quotes::{LEN_SEP, LITERAL_MARK};

/// Build a `\x1D<offset>\x1D` JUMP marker.
fn jmp(offset: u32) -> Vec<u8> {
    let mut v = vec![JUMP_MARK];
    v.extend_from_slice(offset.to_string().as_bytes());
    v.push(JUMP_MARK);
    v
}

/// Build a `\x1F<idx>\x1E<orig_byte_len>` LITERAL marker (matches the form emitted by
/// `mask_quotes`; used to construct synthetic post-mask inputs for round-trip tests).
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

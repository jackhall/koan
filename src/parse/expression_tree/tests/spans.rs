//! Phase-4 span-population tests. Each case parses a snippet and asserts the absolute
//! `Span` recorded on the `KExpression`, its wrapping `Spanned<ExpressionPart>`, and on
//! the individual parts. Spans are inclusive-start / exclusive-end byte offsets into the
//! original source. The helper `parts_with_spans` returns a flat `(label, span)` list so
//! assertions read like a span ledger.

use crate::machine::core::source::{Span, Spanned, SourceFile};
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::parse::expression_tree::parse;

fn span_of(expr: &KExpression<'_>) -> Option<Span> {
    expr.span
}

fn s(start: u32, end: u32) -> Span {
    Span { start, end }
}

/// Top-level parse of `src`. Panics on parse error so tests stay terse.
fn top(src: &str) -> Vec<KExpression<'_>> {
    parse(src).expect("parse")
}

#[test]
fn single_line_top_level_expression_carries_full_span() {
    let exprs = top("foo bar");
    assert_eq!(exprs.len(), 1);
    let e = &exprs[0];
    assert_eq!(span_of(e), Some(s(0, 7)));
    let part_spans: Vec<_> = e.parts.iter().map(|p| p.span).collect();
    assert_eq!(part_spans, vec![Some(s(0, 3)), Some(s(4, 7))]);
}

#[test]
fn nested_call_carries_inner_span() {
    // `foo (bar baz)` — the inner paren expression spans `(bar baz)`.
    let exprs = top("foo (bar baz)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 13)));
    // outer.parts: [Id(foo, (0,3)), Expression(inner, (4,13))]
    assert_eq!(outer.parts[0].span, Some(s(0, 3)));
    assert_eq!(outer.parts[1].span, Some(s(4, 13)));
    let Spanned { value: ExpressionPart::Expression(inner), .. } = &outer.parts[1] else {
        panic!("expected nested Expression part");
    };
    assert_eq!(inner.span, Some(s(4, 13)));
    assert_eq!(inner.parts[0].span, Some(s(5, 8)));
    assert_eq!(inner.parts[1].span, Some(s(9, 12)));
}

#[test]
fn multi_line_top_level_uses_original_byte_offsets() {
    // `foo\nbar` — `bar` starts at byte 4. Collapse strips the newline but the JUMP
    // anchor re-aligns the cursor before the second line's content.
    let exprs = top("foo\nbar");
    assert_eq!(exprs.len(), 2);
    assert_eq!(span_of(&exprs[0]), Some(s(0, 3)));
    assert_eq!(span_of(&exprs[1]), Some(s(4, 7)));
}

#[test]
fn attr_token_spans_full_token_and_trigger_is_one_byte() {
    // `foo.bar` — outer Expression covers the whole token; inner Keyword(ATTR) is 1 byte
    // at the `.`. Individual identifier sub-atoms get their own substring spans.
    let exprs = top("foo.bar");
    let outer = &exprs[0];
    // After peeling, the top-level KExpression *is* the attr expression.
    assert_eq!(span_of(outer), Some(s(0, 7)));
    let kw = &outer.parts[0];
    let lhs = &outer.parts[1];
    let rhs = &outer.parts[2];
    assert!(matches!(kw.value, ExpressionPart::Keyword(ref k) if k == "ATTR"));
    assert_eq!(kw.span, Some(s(3, 4)));
    assert_eq!(lhs.span, Some(s(0, 3)));
    assert_eq!(rhs.span, Some(s(4, 7)));
}

#[test]
fn chained_attr_sub_atoms_get_distinct_trigger_spans() {
    // `foo.bar.baz` — the outer ATTR's trigger is the second `.` (at byte 7); the inner
    // ATTR's trigger is the first `.` (at byte 3).
    let exprs = top("foo.bar.baz");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 11)));
    assert!(matches!(outer.parts[0].value, ExpressionPart::Keyword(ref k) if k == "ATTR"));
    assert_eq!(outer.parts[0].span, Some(s(7, 8)));
    // outer.parts[1] is the inner Expression with its own ATTR
    let Spanned { value: ExpressionPart::Expression(inner), .. } = &outer.parts[1] else {
        panic!("expected nested ATTR Expression");
    };
    assert_eq!(inner.parts[0].span, Some(s(3, 4)));
    assert_eq!(outer.parts[2].span, Some(s(8, 11)));
}

#[test]
fn prefix_negation_trigger_is_one_byte_span() {
    // `!foo` — Keyword(NOT) at (0,1), operand at (1,4), outer wraps both.
    let exprs = top("!foo");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 4)));
    assert!(matches!(outer.parts[0].value, ExpressionPart::Keyword(ref k) if k == "NOT"));
    assert_eq!(outer.parts[0].span, Some(s(0, 1)));
    assert_eq!(outer.parts[1].span, Some(s(1, 4)));
}

#[test]
fn list_literal_wrapper_spans_brackets_inclusive() {
    let exprs = top("[1 2 3]");
    // Top-level expression has 1 part = ListLiteral. The outer Spanned wraps with its
    // full bracket span.
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 7)));
    let part = &outer.parts[0];
    assert_eq!(part.span, Some(s(0, 7)));
    assert!(matches!(part.value, ExpressionPart::ListLiteral(_)));
}

#[test]
fn dict_literal_wrapper_spans_braces_inclusive() {
    let exprs = top("{a: 1}");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 6)));
    let part = &outer.parts[0];
    assert_eq!(part.span, Some(s(0, 6)));
    assert!(matches!(part.value, ExpressionPart::DictLiteral(_)));
}

#[test]
fn quote_sigil_outer_covers_hash_and_body_inner_keyword_one_byte() {
    // `#(foo)` — outer wrapper span = (0, 6). The body Expression spans `(foo)` = (1, 6).
    // The synthetic Keyword("QUOTE") gets a 1-byte span at the `#`.
    let exprs = top("#(foo)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 6)));
    let kw = &outer.parts[0];
    assert!(matches!(kw.value, ExpressionPart::Keyword(ref k) if k == "QUOTE"));
    assert_eq!(kw.span, Some(s(0, 1)));
    let body_part = &outer.parts[1];
    assert_eq!(body_part.span, Some(s(1, 6)));
    let Spanned { value: ExpressionPart::Expression(body), .. } = body_part else {
        panic!("expected body Expression");
    };
    assert_eq!(body.span, Some(s(1, 6)));
    assert_eq!(body.parts[0].span, Some(s(2, 5))); // `foo`
}

#[test]
fn string_literal_span_includes_quotes() {
    // `'hello'` — Literal Spanned wrapper covers both quotes.
    let exprs = top("'hello'");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 7)));
    assert_eq!(outer.parts[0].span, Some(s(0, 7)));
}

#[test]
fn multi_byte_literal_span_counts_bytes_not_codepoints() {
    // `'héllo'` — é is 2 bytes in UTF-8, so the closing quote sits at byte 7 and the
    // exclusive end of the literal span is 8.
    let exprs = top("'héllo'");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 8)));
    assert_eq!(outer.parts[0].span, Some(s(0, 8)));
}

#[test]
fn empty_string_literal_span_covers_both_quotes() {
    let exprs = top("''");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 2)));
    assert_eq!(outer.parts[0].span, Some(s(0, 2)));
}

#[test]
fn peel_redundant_keeps_outermost_span() {
    // `((foo bar))` — peel strips the outer single-Expression wrapper, but the
    // outermost span (0, 11) is restamped on the final survivor.
    let exprs = top("((foo bar))");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 11)));
}

#[test]
fn standalone_lt_keyword_has_one_byte_span() {
    let exprs = top("a < b");
    let outer = &exprs[0];
    assert_eq!(outer.parts[0].span, Some(s(0, 1)));
    assert_eq!(outer.parts[1].span, Some(s(2, 3)));
    assert_eq!(outer.parts[2].span, Some(s(4, 5)));
}

#[test]
fn ascription_compound_keyword_spans_two_bytes() {
    // `name :| Type` — the `:|` keyword is assembled at the build_tree level and spans
    // exactly the glued pair.
    let exprs = top("name :| Type");
    let outer = &exprs[0];
    assert_eq!(outer.parts[0].span, Some(s(0, 4)));
    let kw = &outer.parts[1];
    assert!(matches!(kw.value, ExpressionPart::Keyword(ref k) if k == ":|"));
    assert_eq!(kw.span, Some(s(5, 7)));
}

#[test]
fn type_sigil_paren_wrapper_starts_at_colon() {
    // `:(List Number)` — the Spanned wrapper around the resulting `Type` covers the
    // leading `:` and the closing `)`. span = (0, 14).
    let exprs = top(":(List Number)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 14)));
    assert_eq!(outer.parts[0].span, Some(s(0, 14)));
    assert!(matches!(outer.parts[0].value, ExpressionPart::Type(_)));
}

#[test]
fn span_resolves_to_line_column_via_sourcefile() {
    // End-to-end: build a SourceFile from the same input, register it, and assert that
    // a parsed span resolves to the expected (line, col_utf16).
    let src = "foo\nbar baz";
    let exprs = top(src);
    let file = SourceFile::new("<t>", src.to_string());
    // Second top-level expression starts at byte 4 = `bar` on line 2.
    assert_eq!(file.resolve(span_of(&exprs[1]).unwrap().start), (2, 1));
    // `baz` token sits at byte 8.
    assert_eq!(file.resolve(exprs[1].parts[1].span.unwrap().start), (2, 5));
}

#[test]
fn literal_inside_call_preserves_outer_span_and_string_value() {
    // `(say 'hi')` — outer Expression covers everything; inner literal has its own span
    // and its string value is unchanged by the cursor plumbing.
    let exprs = top("(say 'hi')");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 10)));
    let lit_part = &outer.parts[1];
    assert_eq!(lit_part.span, Some(s(5, 9)));
    let ExpressionPart::Literal(KLiteral::String(ref s)) = lit_part.value else {
        panic!("expected string literal");
    };
    assert_eq!(s, "hi");
}

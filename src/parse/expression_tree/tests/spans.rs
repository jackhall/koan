//! Span-population tests. Spans are inclusive-start / exclusive-end byte offsets
//! into the original source.

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::parse::expression_tree::{parse, parse_with_path};
use crate::source::{self, SourceFile, Span, Spanned};

fn span_of(expr: &KExpression<'_>) -> Option<Span> {
    expr.span
}

fn s(start: u32, end: u32) -> Span {
    Span { start, end }
}

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
    let exprs = top("foo (bar baz)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 13)));
    assert_eq!(outer.parts[0].span, Some(s(0, 3)));
    assert_eq!(outer.parts[1].span, Some(s(4, 13)));
    let Spanned {
        value: ExpressionPart::Expression(inner),
        ..
    } = &outer.parts[1]
    else {
        panic!("expected nested Expression part");
    };
    assert_eq!(inner.span, Some(s(4, 13)));
    assert_eq!(inner.parts[0].span, Some(s(5, 8)));
    assert_eq!(inner.parts[1].span, Some(s(9, 12)));
}

#[test]
fn multi_line_top_level_uses_original_byte_offsets() {
    // Collapse strips the newline but the JUMP anchor re-aligns the cursor before line 2.
    let exprs = top("foo\nbar");
    assert_eq!(exprs.len(), 2);
    assert_eq!(span_of(&exprs[0]), Some(s(0, 3)));
    assert_eq!(span_of(&exprs[1]), Some(s(4, 7)));
}

#[test]
fn attr_token_spans_full_token_and_trigger_is_one_byte() {
    let exprs = top("foo.bar");
    let outer = &exprs[0];
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
    let exprs = top("foo.bar.baz");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 11)));
    assert!(matches!(outer.parts[0].value, ExpressionPart::Keyword(ref k) if k == "ATTR"));
    assert_eq!(outer.parts[0].span, Some(s(7, 8)));
    let Spanned {
        value: ExpressionPart::Expression(inner),
        ..
    } = &outer.parts[1]
    else {
        panic!("expected nested ATTR Expression");
    };
    assert_eq!(inner.parts[0].span, Some(s(3, 4)));
    assert_eq!(outer.parts[2].span, Some(s(8, 11)));
}

#[test]
fn list_literal_wrapper_spans_brackets_inclusive() {
    let exprs = top("[1 2 3]");
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

/// The captured part covers the `#` plus the group; the body keeps the paren span.
#[test]
fn quote_part_covers_hash_and_body_keeps_paren_span() {
    let exprs = top("#(foo)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 6)));
    let quoted = &outer.parts[0];
    assert_eq!(quoted.span, Some(s(0, 6)));
    let Spanned {
        value: ExpressionPart::QuotedExpression(body),
        ..
    } = quoted
    else {
        panic!("expected a QuotedExpression part");
    };
    assert_eq!(body.span, Some(s(1, 6)));
    assert_eq!(body.parts[0].span, Some(s(2, 5)));
}

#[test]
fn string_literal_span_includes_quotes() {
    let exprs = top("'hello'");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 7)));
    assert_eq!(outer.parts[0].span, Some(s(0, 7)));
}

#[test]
fn multi_byte_literal_span_counts_bytes_not_codepoints() {
    // é is 2 bytes in UTF-8 — closing quote at byte 7, exclusive end 8.
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
    // Peel strips the outer wrapper, but the outermost span is restamped on the survivor.
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
    let exprs = top("name :| Type");
    let outer = &exprs[0];
    assert_eq!(outer.parts[0].span, Some(s(0, 4)));
    let kw = &outer.parts[1];
    assert!(matches!(kw.value, ExpressionPart::Keyword(ref k) if k == ":|"));
    assert_eq!(kw.span, Some(s(5, 7)));
}

#[test]
fn type_sigil_paren_wrapper_starts_at_colon() {
    let exprs = top(":(List Number)");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 14)));
    assert_eq!(outer.parts[0].span, Some(s(0, 14)));
    assert!(matches!(
        outer.parts[0].value,
        ExpressionPart::SigiledTypeExpr(_)
    ));
}

#[test]
fn span_resolves_to_line_column_via_sourcefile() {
    let src = "foo\nbar baz";
    let exprs = top(src);
    let file = SourceFile::new("<t>", src.to_string());
    assert_eq!(file.resolve(span_of(&exprs[1]).unwrap().start), (2, 1));
    assert_eq!(file.resolve(exprs[1].parts[1].span.unwrap().start), (2, 5));
}

#[test]
fn parse_with_path_stamps_file_on_expression_and_resolves_line_col() {
    // Line layout (with leading offsets):
    //   line 1:  `foo (`                  byte 0..5
    //   line 2:  `  bar`                  byte 6..11
    //   line 3:  `    (qux))`             byte 14..23  (inner `(qux)` at byte 18, col 5)
    let src = "foo (\n  bar\n    (qux))";
    let exprs = parse_with_path(src, "lib.koan").expect("parse");
    let outer = &exprs[0];
    let inner = match &outer.parts.last().expect("outer has parts").value {
        ExpressionPart::Expression(e) => &**e,
        other => panic!("expected continuation Expression part, got {other:?}"),
    };
    let nested = match &inner.parts.last().expect("inner has parts").value {
        ExpressionPart::Expression(e) => &**e,
        other => panic!("expected nested Expression part, got {other:?}"),
    };
    let file_id = nested
        .file
        .expect("file should be populated by parse_with_path");
    let span = nested.span.expect("span should be populated");
    let (line, col) = source::with(file_id, |f| {
        assert_eq!(&*f.path, "lib.koan");
        f.resolve(span.start)
    });
    assert_eq!((line, col), (3, 5));
}

#[test]
fn literal_inside_call_preserves_outer_span_and_string_value() {
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

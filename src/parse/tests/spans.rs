//! The spans a parse synthesizes rather than reads off a token: an operator trigger folded out
//! of a compound atom, a compound keyword, and the file a span resolves against. That every other
//! span indexes its own text is [`properties`](super::properties)' fourth law.

use crate::memory::{ProgramBrand, program_storage};
use crate::parse::{ExpressionPart, KExpression};
use crate::parse::{parse, parse_with_path};
use crate::source::{self, SourceFile, Span, Spanned};

fn span_of(expr: &KExpression<'_>) -> Option<Span> {
    expr.span
}

fn s(start: u32, end: u32) -> Span {
    Span { start, end }
}

fn top<'a>(brand: ProgramBrand<'a>, src: &str) -> Vec<KExpression<'a>> {
    parse(brand, &crate::parse::LabelInterner::new(), src).expect("parse")
}

#[test]
fn attr_token_spans_full_token_and_trigger_is_one_byte() {
    let program = program_storage();
    let exprs = top(program.brand(), "foo.bar");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 7)));
    let kw = &outer.parts[0];
    let lhs = &outer.parts[1];
    let rhs = &outer.parts[2];
    assert!(matches!(kw.value, ExpressionPart::Keyword(symbol) if symbol == probe_symbol("ATTR")));
    assert_eq!(kw.span, Some(s(3, 4)));
    assert_eq!(lhs.span, Some(s(0, 3)));
    assert_eq!(rhs.span, Some(s(4, 7)));
}

#[test]
fn chained_attr_sub_atoms_get_distinct_trigger_spans() {
    let program = program_storage();
    let exprs = top(program.brand(), "foo.bar.baz");
    let outer = &exprs[0];
    assert_eq!(span_of(outer), Some(s(0, 11)));
    assert!(
        matches!(outer.parts[0].value, ExpressionPart::Keyword(symbol) if symbol == probe_symbol("ATTR"))
    );
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
fn ascription_compound_keyword_spans_two_bytes() {
    let program = program_storage();
    let exprs = top(program.brand(), "name :| Type");
    let outer = &exprs[0];
    assert_eq!(outer.parts[0].span, Some(s(0, 4)));
    let kw = &outer.parts[1];
    assert!(matches!(kw.value, ExpressionPart::Keyword(symbol) if symbol == probe_symbol(":|")));
    assert_eq!(kw.span, Some(s(5, 7)));
}

#[test]
fn span_resolves_to_line_column_via_sourcefile() {
    let src = "foo\nbar baz";
    let program = program_storage();
    let exprs = top(program.brand(), src);
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
    let program = program_storage();
    let exprs = parse_with_path(
        program.brand(),
        &crate::parse::LabelInterner::new(),
        src,
        "lib.koan",
    )
    .expect("parse");
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

/// The operator-probe symbol for a probe key a test spells out (`"ATTR"`, `":|"`).
fn probe_symbol(text: &str) -> crate::parse::labels::KeywordSymbol {
    crate::parse::labels::KeywordSymbol::of(text)
        .expect("a test fixture operator probe is keyword-class")
}

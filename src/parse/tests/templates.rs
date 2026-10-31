//! Templates: `%` glued to a string, its `{…}` and `%{…}` braces each lowered as a paren group
//! with the same contents, `{{` and `}}` read as one brace, and the errors a malformed brace is
//! refused with, each spanning what it names.

use crate::memory::program_storage;
use crate::parse::{ExpressionPart, parse};
use crate::source::Span;
use crate::symbols::SymbolInterner;

use super::tree;

#[test]
fn a_template_holds_its_text_and_braces() {
    assert_eq!(tree(r#"%"a{x}b""#).unwrap(), "[T[s(a) {[t(x)]} s(b)]]");
    assert_eq!(tree("%'a%{x}b'").unwrap(), "[T[s(a) %{[t(x)]} s(b)]]");
    assert_eq!(tree(r#"%"a""#).unwrap(), "[T[s(a)]]");
    assert_eq!(tree(r#"%"""#).unwrap(), "[T[s()]]");
}

#[test]
fn doubled_braces_are_text() {
    assert_eq!(tree(r#"%"{{x}} %{{""#).unwrap(), "[T[s({x} %{)]]");
    assert_eq!(tree(r#"%"{x}}}""#).unwrap(), "[T[s() {[t(x)]} s(})]]");
}

#[test]
fn a_brace_holds_one_expression_as_a_paren_does() {
    assert_eq!(
        tree(r#"%"{a + b}""#).unwrap(),
        "[T[s() {[t(a) t(+) t(b)]} s()]]"
    );
    // A brace whose expression starts with `{` needs a space, since `{{` is text.
    assert_eq!(
        tree(r#"%"{ {a = 1} }""#).unwrap(),
        "[T[s() {[R{a = n(1)}]} s()]]"
    );
    assert_eq!(
        tree(r#"%"{(f %'{x}')}""#).unwrap(),
        "[T[s() {[[t(f) T[s() {[t(x)]} s()]]]} s()]]"
    );
}

#[test]
fn a_malformed_brace_is_refused_where_it_is_written() {
    let refused = |source: &str| {
        let program = program_storage();
        let error = parse(program.brand(), &SymbolInterner::new(), source).unwrap_err();
        (error.message.clone(), error.span)
    };
    let span = |start, end| Some(Span { start, end });
    let (message, at) = refused(r#"%"a}b""#);
    assert!(message.contains("written `}}`"), "{message}");
    assert_eq!(at, span(3, 4));
    let (message, at) = refused(r#"%"a{x b""#);
    assert!(message.contains("no closing"), "{message}");
    assert_eq!(at, span(3, 7));
    let (message, at) = refused(r#"%"a{ }""#);
    assert!(message.contains("holds an expression"), "{message}");
    assert_eq!(at, span(3, 6));
}

/// Composing stops at a literal, so a splice in a group a literal holds, or in a template's brace,
/// is refused at the group or brace; one in a quote nested there belongs to that quote.
#[test]
fn a_splice_under_a_literal_is_refused() {
    let refused = |source: &str| {
        let program = program_storage();
        let error = parse(program.brand(), &SymbolInterner::new(), source).unwrap_err();
        assert!(
            error.message.contains("a template's brace is none"),
            "{}",
            error.message
        );
        error.span
    };
    let span = |start, end| Some(Span { start, end });
    assert_eq!(refused(r#"#(%"a{$..p}")"#), span(5, 11));
    assert_eq!(refused("#([($..p)])"), span(3, 9));
    assert_eq!(refused("#({a = (f ($..p))})"), span(7, 17));
    assert!(tree(r#"#(%"{#($..p)}")"#).is_ok());
    assert!(tree("#([#($..p)])").is_ok());
}

#[test]
fn only_a_glued_percent_makes_a_template() {
    assert_eq!(tree(r#"% "x""#).unwrap(), "[t(%) s(x)]");
    assert!(!tree(r#"x%"y""#).unwrap_or_default().contains("T["));
}

#[test]
fn a_brace_node_spans_its_text_in_the_file() {
    let program = program_storage();
    let source = r#"PRINT %"ab{x + 1}c""#;
    let lines = parse(program.brand(), &SymbolInterner::new(), source).unwrap();
    let ExpressionPart::Template(template) = lines[0].parts[1].value else {
        panic!("a template")
    };
    assert_eq!(
        lines[0].parts[1].span.unwrap().slice(source),
        r#"%"ab{x + 1}c""#
    );
    let ExpressionPart::Expression(node) = template.slots[0].part() else {
        panic!("a brace holds a node")
    };
    assert_eq!(node.source.span.slice(source), "{x + 1}");
    assert_eq!(node.parts[0].span.unwrap().slice(source), "x");
}

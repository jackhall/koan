//! Shared parser for `<name>: <value> <name>: <value> ...` named-argument lists. Used by
//! struct construction ([`crate::runtime::builtins::struct_value::apply`]) and first-class
//! function calls ([`KFunction::apply`](crate::runtime::machine::kfunction::KFunction)) —
//! the two paths that switched from positional to named arguments.
//!
//! Mirrors the shape of [`crate::runtime::model::types::parse_typed_field_list_via_elaborator`];
//! both parsers walk the same `<Identifier> : <slot>` triple shape and share the
//! identifier/colon/duplicate scaffolding through [`crate::parse::parse_triple_list`].
//! The third-slot interpretation is the only thing that differs — value-side here is
//! "take the part verbatim", type-side over there is "resolve as a KType against scope".
//! The wrapper closes over the right interpretation.

use crate::ast::{ExpressionPart, KExpression};
use crate::parse::parse_triple_list;

/// Walk an expression's parts as repeated `<Identifier(name)> <Keyword(":")> <value>` triples
/// and assemble the resulting ordered list of `(name, value-part)` pairs. Errors with a
/// `ShapeError`-string on any malformed triple or duplicate name. `context` is the
/// surface-form description (`"struct construction"`, `"function call"`) used in error
/// messages so the caller's diagnostic stays grounded in user-facing syntax.
///
/// Empty `parts` returns an empty Vec — supports zero-arg calls like `f ()`.
///
/// Thin wrapper over [`parse_triple_list`] that closes over "take the third part as-is".
/// The shared helper's error messages name the slot `<slot>`; this wrapper accepts that
/// since the value side never rejects on the slot's content (only on shape), so the third
/// closure is `Ok(part.clone())` unconditionally.
pub fn parse_named_value_pairs<'a>(
    expr: &KExpression<'a>,
    context: &str,
) -> Result<Vec<(String, ExpressionPart<'a>)>, String> {
    parse_triple_list(expr, context, |part, _name| Ok(part.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::KLiteral;

    fn ident(s: &str) -> ExpressionPart<'static> {
        ExpressionPart::Identifier(s.to_string())
    }
    fn colon() -> ExpressionPart<'static> {
        ExpressionPart::Keyword(":".to_string())
    }
    fn num(n: f64) -> ExpressionPart<'static> {
        ExpressionPart::Literal(KLiteral::Number(n))
    }

    #[test]
    fn empty_parts_returns_empty_vec() {
        let expr = KExpression { parts: vec![] };
        let pairs = parse_named_value_pairs(&expr, "ctx").unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn single_pair_round_trips() {
        let expr = KExpression {
            parts: vec![ident("x"), colon(), num(3.0)],
        };
        let pairs = parse_named_value_pairs(&expr, "ctx").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "x");
        assert!(matches!(pairs[0].1, ExpressionPart::Literal(KLiteral::Number(n)) if n == 3.0));
    }

    #[test]
    fn multiple_pairs_preserve_order() {
        let expr = KExpression {
            parts: vec![
                ident("y"), colon(), num(4.0),
                ident("x"), colon(), num(3.0),
            ],
        };
        let pairs = parse_named_value_pairs(&expr, "ctx").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "y");
        assert_eq!(pairs[1].0, "x");
    }

    #[test]
    fn duplicate_name_errors() {
        let expr = KExpression {
            parts: vec![
                ident("x"), colon(), num(1.0),
                ident("x"), colon(), num(2.0),
            ],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("duplicate name"), "got: {err}");
        assert!(err.contains("`x`"), "got: {err}");
    }

    #[test]
    fn missing_colon_errors() {
        let expr = KExpression {
            parts: vec![ident("x"), num(3.0), ident("y")],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("`:`") || err.contains("separator"), "got: {err}");
    }

    #[test]
    fn non_identifier_name_errors() {
        let expr = KExpression {
            parts: vec![num(7.0), colon(), num(3.0)],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("bare identifier"), "got: {err}");
    }

    #[test]
    fn non_multiple_of_three_errors() {
        let expr = KExpression {
            parts: vec![ident("x"), colon()],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("triples"), "got: {err}");
    }
}

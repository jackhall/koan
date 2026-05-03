//! Shared parser for `<name>: <value> <name>: <value> ...` named-argument lists. Used by
//! struct construction ([`struct_value::apply`](super::struct_value::apply)) and first-class
//! function calls ([`KFunction::apply`](super::kfunction::KFunction::apply)) — the two paths
//! that switched from positional to named arguments.
//!
//! Mirrors the shape of [`typed_field_list::parse_typed_field_list`](super::typed_field_list::parse_typed_field_list)
//! but the third element of each triple is an arbitrary value-side `ExpressionPart` rather
//! than a `KType` token. The parser leaves the value untouched — the caller decides how to
//! resolve it (wrap-and-dispatch for eager evaluation, or thread it through unchanged).

use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Walk an expression's parts as repeated `<Identifier(name)> <Keyword(":")> <value>` triples
/// and assemble the resulting ordered list of `(name, value-part)` pairs. Errors with a
/// `ShapeError`-string on any malformed triple or duplicate name. `context` is the
/// surface-form description (`"struct construction"`, `"function call"`) used in error
/// messages so the caller's diagnostic stays grounded in user-facing syntax.
///
/// Empty `parts` returns an empty Vec — supports zero-arg calls like `f ()`.
pub fn parse_named_value_pairs<'a>(
    expr: &KExpression<'a>,
    context: &str,
) -> Result<Vec<(String, ExpressionPart<'a>)>, String> {
    let parts = &expr.parts;
    if parts.len() % 3 != 0 {
        return Err(format!(
            "{context} args must be `<name>: <value>` triples; got {} parts (not a multiple of 3)",
            parts.len(),
        ));
    }
    let mut pairs: Vec<(String, ExpressionPart<'a>)> = Vec::with_capacity(parts.len() / 3);
    let mut i = 0;
    while i < parts.len() {
        let name = match &parts[i] {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} arg name must be a bare identifier, got {}",
                    other.summarize(),
                ));
            }
        };
        match &parts[i + 1] {
            ExpressionPart::Keyword(k) if k == ":" => {}
            other => {
                return Err(format!(
                    "{context} separator must be `:`, got {}",
                    other.summarize(),
                ));
            }
        }
        if pairs.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context}", name));
        }
        pairs.push((name, parts[i + 2].clone()));
        i += 3;
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::kexpression::KLiteral;

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

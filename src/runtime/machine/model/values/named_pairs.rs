//! Shared parser for named-argument lists. Used by struct construction
//! ([`crate::runtime::builtins::struct_value::apply`]) and first-class function calls
//! ([`KFunction::apply`](crate::runtime::machine::core::kfunction::KFunction)) — the two
//! paths that switched from positional to named arguments.
//!
//! Two surface forms are accepted:
//!
//! - `Point (x = 3, y = 4)` — paren-expr with `=`-separated triples. The inner expression
//!   parts are walked as `<Identifier> <Keyword("=")> <value>` triples via
//!   [`parse_keyword_triple_list`].
//! - `Point {x: 3, y: 4}` — single dict literal whose string keys are the field names.
//!   The dict-frame `:` keeps its pair-separator role, so this form is the natural
//!   surface for users coming from dict literals.
//!
//! The parser inspects the input shape: a single `ExpressionPart::DictLiteral` chooses
//! the dict-form path; anything else routes through the keyword-triple walker.

use crate::runtime::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::parse::parse_keyword_triple_list;

/// Walk an expression's parts as a named-value list and assemble the resulting ordered
/// list of `(name, value-part)` pairs. Errors with a `ShapeError`-string on malformed
/// shapes or duplicate names.
///
/// Accepts either form (see module docs); empty `parts` returns an empty `Vec`.
pub fn parse_named_value_pairs<'a>(
    expr: &KExpression<'a>,
    context: &str,
) -> Result<Vec<(String, ExpressionPart<'a>)>, String> {
    if let [ExpressionPart::DictLiteral(pairs)] = expr.parts.as_slice() {
        let mut out: Vec<(String, ExpressionPart<'a>)> = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            let name = match key {
                ExpressionPart::Identifier(s) => s.clone(),
                ExpressionPart::Literal(KLiteral::String(s)) => s.clone(),
                other => {
                    return Err(format!(
                        "{context} dict-form key must be a bare identifier or string, got {}",
                        other.summarize(),
                    ));
                }
            };
            if out.iter().any(|(n, _)| n == &name) {
                return Err(format!("duplicate name `{}` in {context}", name));
            }
            out.push((name, value.clone()));
        }
        return Ok(out);
    }
    parse_keyword_triple_list(expr, context, "=", |part, _name| Ok(part.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::machine::model::ast::KLiteral;

    fn ident(s: &str) -> ExpressionPart<'static> {
        ExpressionPart::Identifier(s.to_string())
    }
    fn eq_kw() -> ExpressionPart<'static> {
        ExpressionPart::Keyword("=".to_string())
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
    fn single_eq_pair_round_trips() {
        let expr = KExpression {
            parts: vec![ident("x"), eq_kw(), num(3.0)],
        };
        let pairs = parse_named_value_pairs(&expr, "ctx").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "x");
        assert!(matches!(pairs[0].1, ExpressionPart::Literal(KLiteral::Number(n)) if n == 3.0));
    }

    #[test]
    fn multiple_eq_pairs_preserve_order() {
        let expr = KExpression {
            parts: vec![
                ident("y"), eq_kw(), num(4.0),
                ident("x"), eq_kw(), num(3.0),
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
                ident("x"), eq_kw(), num(1.0),
                ident("x"), eq_kw(), num(2.0),
            ],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("duplicate name"), "got: {err}");
        assert!(err.contains("`x`"), "got: {err}");
    }

    #[test]
    fn missing_eq_separator_errors() {
        let expr = KExpression {
            parts: vec![ident("x"), num(3.0), ident("y")],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("`=`") || err.contains("separator"), "got: {err}");
    }

    #[test]
    fn non_identifier_name_errors() {
        let expr = KExpression {
            parts: vec![num(7.0), eq_kw(), num(3.0)],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("bare identifier"), "got: {err}");
    }

    #[test]
    fn non_multiple_of_three_errors() {
        let expr = KExpression {
            parts: vec![ident("x"), eq_kw()],
        };
        let err = parse_named_value_pairs(&expr, "ctx").unwrap_err();
        assert!(err.contains("triples"), "got: {err}");
    }

    #[test]
    fn dict_literal_form_round_trips() {
        let expr = KExpression {
            parts: vec![ExpressionPart::DictLiteral(vec![
                (ident("x"), num(3.0)),
                (ident("y"), num(4.0)),
            ])],
        };
        let pairs = parse_named_value_pairs(&expr, "ctx").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "x");
        assert_eq!(pairs[1].0, "y");
    }
}

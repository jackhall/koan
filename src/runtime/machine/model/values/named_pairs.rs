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
//!
//! After parsing, [`NamedPairs`] wraps the resulting name→value map as a consume-by-name
//! handle: callers `take(name)` for each declared slot, and any residual entry surfaces
//! via [`NamedPairs::into_unknown`] as the unknown-name error. The wrapper encodes the
//! presence-once invariant the call sites previously enforced via three passes plus a
//! `.expect("missing-arg check above guarantees presence")`.

use std::collections::HashMap;

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

/// Consume-by-name view over a named-argument list. Built from
/// [`parse_named_value_pairs`]; callers `take(name)` for each declared slot and call
/// [`into_unknown`](Self::into_unknown) at the end to surface any unconsumed name.
///
/// Duplicate names are rejected during parsing, so the map is bijective: each `take`
/// either returns the unique value or yields a missing-name error. Arity is implicit —
/// once every declared name has been taken and the residual is empty, the input
/// matched the declaration exactly.
pub struct NamedPairs<'a> {
    map: HashMap<String, ExpressionPart<'a>>,
}

impl<'a> NamedPairs<'a> {
    /// Parse `expr` as a named-value list and wrap it for consume-by-name access.
    pub fn parse(expr: &KExpression<'a>, context: &str) -> Result<Self, String> {
        let pairs = parse_named_value_pairs(expr, context)?;
        Ok(Self { map: pairs.into_iter().collect() })
    }

    /// Pop the value bound to `name`, or `None` if the caller did not provide it.
    pub fn take(&mut self, name: &str) -> Option<ExpressionPart<'a>> {
        self.map.remove(name)
    }

    /// Return the name of an arbitrary unconsumed entry, or `None` if the map is empty.
    /// Call after all declared slots have been [`take`](Self::take)n; a `Some` indicates
    /// the caller supplied a name the declaration did not expect.
    pub fn into_unknown(self) -> Option<String> {
        self.map.into_keys().next()
    }
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
    fn named_pairs_take_consumes_by_name() {
        let expr = KExpression {
            parts: vec![
                ident("x"), eq_kw(), num(3.0),
                ident("y"), eq_kw(), num(4.0),
            ],
        };
        let mut pairs = NamedPairs::parse(&expr, "ctx").unwrap();
        assert!(matches!(pairs.take("y"), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 4.0));
        assert!(matches!(pairs.take("x"), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 3.0));
        assert!(pairs.take("y").is_none(), "second take returns None");
        assert!(pairs.into_unknown().is_none(), "all entries consumed");
    }

    #[test]
    fn named_pairs_into_unknown_reports_residual() {
        let expr = KExpression {
            parts: vec![ident("x"), eq_kw(), num(3.0), ident("z"), eq_kw(), num(9.0)],
        };
        let mut pairs = NamedPairs::parse(&expr, "ctx").unwrap();
        let _ = pairs.take("x");
        assert_eq!(pairs.into_unknown().as_deref(), Some("z"));
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

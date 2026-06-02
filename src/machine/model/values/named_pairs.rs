//! Consume-by-name view over a call's named arguments — the fields of a `{name = value}`
//! record literal, reordered into a declaration's parameter / field order by struct
//! construction ([`struct_value`](crate::machine::execute)) and function / functor calls
//! ([`KFunction::reconstruct_positional`](crate::machine::core::kfunction::KFunction)).

use std::collections::HashMap;

use crate::machine::model::ast::ExpressionPart;

/// Consume-by-name view over a named-argument list. Callers `take(name)` for each
/// declared slot and call [`into_unknown`](Self::into_unknown) at the end to surface any
/// unconsumed name. Built from a record literal's `(name, value)` fields.
#[derive(Debug)]
pub struct NamedPairs<'a> {
    map: HashMap<String, ExpressionPart<'a>>,
}

impl<'a> NamedPairs<'a> {
    /// Wrap a record literal's fields for consume-by-name access. Errors on a duplicate
    /// field name: a record *value* last-wins on duplicates, but a named-argument list
    /// must reject them so a doubly-supplied argument fails loudly rather than silently.
    pub fn from_fields(fields: Vec<(String, ExpressionPart<'a>)>) -> Result<Self, String> {
        let mut map = HashMap::with_capacity(fields.len());
        for (name, value) in fields {
            if map.insert(name.clone(), value).is_some() {
                return Err(format!("duplicate name `{name}`"));
            }
        }
        Ok(Self { map })
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
    use crate::machine::model::ast::KLiteral;

    fn num(n: f64) -> ExpressionPart<'static> {
        ExpressionPart::Literal(KLiteral::Number(n))
    }

    #[test]
    fn take_consumes_by_name() {
        let mut pairs =
            NamedPairs::from_fields(vec![("x".into(), num(3.0)), ("y".into(), num(4.0))]).unwrap();
        assert!(
            matches!(pairs.take("y"), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 4.0)
        );
        assert!(
            matches!(pairs.take("x"), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 3.0)
        );
        assert!(pairs.take("y").is_none(), "second take returns None");
        assert!(pairs.into_unknown().is_none(), "all entries consumed");
    }

    #[test]
    fn into_unknown_reports_residual() {
        let mut pairs =
            NamedPairs::from_fields(vec![("x".into(), num(3.0)), ("z".into(), num(9.0))]).unwrap();
        let _ = pairs.take("x");
        assert_eq!(pairs.into_unknown().as_deref(), Some("z"));
    }

    #[test]
    fn duplicate_name_errors() {
        let err = NamedPairs::from_fields(vec![("x".into(), num(1.0)), ("x".into(), num(2.0))])
            .unwrap_err();
        assert!(err.contains("duplicate name"), "got: {err}");
        assert!(err.contains("`x`"), "got: {err}");
    }
}

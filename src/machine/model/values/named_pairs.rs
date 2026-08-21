//! Consume-by-name view over a call's named arguments — the fields of a `{name = value}`
//! record literal, reordered into a declaration's parameter / field order by struct
//! construction ([`struct_value`](crate::machine::execute)) and function calls
//! ([`KFunction::reconstruct_positional`](crate::machine::core::KFunction)).

use std::collections::HashMap;

use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::labels::Symbol;

/// Consume-by-name view over a named-argument list. Callers `take(name)` for each
/// declared slot; leftover names are dropped (call-by-name width drop). Built from a record
/// literal's `(name, value)` fields.
#[derive(Debug)]
pub struct NamedPairs<'a> {
    map: HashMap<Symbol, ExpressionPart<'a>>,
}

impl<'a> NamedPairs<'a> {
    /// Wrap a record literal's fields for consume-by-name access. Errors on a duplicate
    /// field name. A parsed record literal is already duplicate-free — the brace-literal
    /// parser rejects a repeated field — so this guards the fields a caller assembles
    /// directly, keeping a doubly-supplied argument loud rather than silent.
    pub fn from_fields(
        fields: Vec<(Symbol, ExpressionPart<'a>)>,
        registries: &crate::machine::model::RunRegistries,
    ) -> Result<Self, String> {
        let mut map = HashMap::with_capacity(fields.len());
        for (name, value) in fields {
            if map.insert(name, value).is_some() {
                return Err(format!(
                    "duplicate name `{}`",
                    crate::machine::model::render_label(name, registries)
                ));
            }
        }
        Ok(Self { map })
    }

    /// Pop the value bound to `name`, or `None` if the caller did not provide it.
    pub fn take(&mut self, name: Symbol) -> Option<ExpressionPart<'a>> {
        self.map.remove(&name)
    }

    /// Whether `name` is still present — [`take`](Self::take)'s non-consuming peer, for a caller
    /// that has to know the whole run is satisfiable before it takes the first value.
    pub fn contains(&self, name: Symbol) -> bool {
        self.map.contains_key(&name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::model::ast::KLiteral;

    fn num(n: f64) -> ExpressionPart<'static> {
        ExpressionPart::Literal(KLiteral::Number(n))
    }

    /// Labels resolve for the duplicate-name diagnostic, so the test interns through a registry of
    /// its own rather than minting bare symbols.
    fn registries() -> crate::machine::model::RunRegistries {
        crate::machine::model::RunRegistries::new()
    }

    #[test]
    fn take_consumes_by_name() {
        let mut pairs = NamedPairs::from_fields(
            vec![(Symbol::of("x"), num(3.0)), (Symbol::of("y"), num(4.0))],
            &registries(),
        )
        .unwrap();
        assert!(
            matches!(pairs.take(Symbol::of("y")), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 4.0)
        );
        assert!(
            matches!(pairs.take(Symbol::of("x")), Some(ExpressionPart::Literal(KLiteral::Number(n))) if n == 3.0)
        );
        assert!(
            pairs.take(Symbol::of("y")).is_none(),
            "second take returns None"
        );
    }

    #[test]
    fn duplicate_name_errors() {
        let registries = registries();
        registries.labels.intern("x");
        let err = NamedPairs::from_fields(
            vec![(Symbol::of("x"), num(1.0)), (Symbol::of("x"), num(2.0))],
            &registries,
        )
        .unwrap_err();
        assert!(err.contains("duplicate name"), "got: {err}");
        assert!(err.contains("`x`"), "got: {err}");
    }
}

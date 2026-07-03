//! Bind a user-defined call's already-resolved positional arguments to a function's parameters — the
//! binder the `exec` body executor uses. A named call (`f {x = a}`) is reordered into positional order
//! by [`KFunction::reconstruct_positional`] before dispatch, so every call reaches this binder as
//! positional values in parameter order. (Builtins bind via [`KFunction::bind_args`], which produces
//! an owned `Record<Held>`.) The arguments arrive as [`Carried`] values (resolved into the region by
//! dispatch), so binding is a pure rename map into a `Record<Carried>`: no owned-value wrapping and no
//! per-argument type-check — that is the picker's job, and the carried type is trusted here.

use super::KFunction;
use crate::machine::model::types::{Argument, ExpressionSignature, Record, SignatureElement};
use crate::machine::model::Carried;
use crate::machine::{KError, KErrorKind};

impl<'a> KFunction<'a> {
    /// Bind `values` to this function's parameters in signature order, producing the argument record
    /// directly. Keywords are the signature's own literals (matched when the overload was picked), so
    /// `values` holds only the value cells.
    pub fn bind_by_name(&'a self, values: Vec<Carried<'a>>) -> Result<Record<Carried<'a>>, KError> {
        bind_args_by_name(&self.signature, values)
    }
}

/// Signature-only core of [`KFunction::bind_by_name`] — needs no body or captured scope, so it is
/// directly testable. Walks the signature's parameters in order, taking each parameter's supplied
/// value by position into the record; too few values is a `MissingArg`.
pub fn bind_args_by_name<'a>(
    signature: &ExpressionSignature<'a>,
    values: Vec<Carried<'a>>,
) -> Result<Record<Carried<'a>>, KError> {
    let mut bound: Record<Carried<'a>> = Record::new();
    let mut values = values.into_iter();
    for arg in arguments(signature) {
        let value = values
            .next()
            .ok_or_else(|| KError::new(KErrorKind::MissingArg(arg.name.clone())))?;
        bound.insert(arg.name.clone(), value);
    }
    Ok(bound)
}

/// The signature's parameters in order — `Keyword` elements are the call form's own literals, not
/// bound values, so they are skipped.
fn arguments<'sig, 'a>(
    signature: &'sig ExpressionSignature<'a>,
) -> impl Iterator<Item = &'sig Argument<'a>> {
    signature.elements.iter().filter_map(|el| match el {
        SignatureElement::Argument(arg) => Some(arg),
        SignatureElement::Keyword(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::FrameStorage;
    use crate::machine::model::types::ReturnType;
    use crate::machine::model::values::KObject;
    use crate::machine::model::KType;

    /// `(DOUBLE x: Number)` — one keyword, one parameter.
    fn double_signature<'a>() -> ExpressionSignature<'a> {
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Number),
            elements: vec![
                SignatureElement::Keyword("DOUBLE".to_string()),
                SignatureElement::Argument(Argument {
                    name: "x".to_string(),
                    ktype: KType::Number,
                }),
            ],
        }
    }

    fn bound_x(bound: &Record<Carried<'_>>) -> f64 {
        let &value = bound.get("x").expect("x is bound");
        match value {
            Carried::Object(KObject::Number(n)) => *n,
            _ => panic!("x should bind a Number"),
        }
    }

    #[test]
    fn positional_binds() {
        let storage = FrameStorage::run_root();
        let region = storage.brand();
        let seven = Carried::Object(region.alloc_object(KObject::Number(7.0)));

        let bound = bind_args_by_name(&double_signature(), vec![seven]).expect("positional binds");
        assert_eq!(bound_x(&bound), 7.0);
    }

    #[test]
    fn missing_parameter_errors() {
        let result = bind_args_by_name(&double_signature(), Vec::new());
        assert!(matches!(
            result,
            Err(e) if matches!(e.kind, KErrorKind::MissingArg(ref n) if n == "x")
        ));
    }
}

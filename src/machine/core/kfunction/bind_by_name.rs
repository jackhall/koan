//! Bind a user-defined call's already-resolved arguments to a function's parameters *by name* — the
//! binder the `exec` body executor uses, subsuming both call forms (named `f {x = a}` and positional
//! `f a b`). (Builtins bind via [`KFunction::bind`], which produces a `Record<ArgValue>`.) The
//! arguments arrive as [`Carried`] values (resolved into the arena by dispatch), so binding is a
//! pure rename map into a `Record<Carried>`: no `ArgValue` wrapping and no per-argument type-check —
//! that is the picker's job, and the carried type is trusted here.

use super::KFunction;
use crate::machine::model::types::{Argument, ExpressionSignature, Record, SignatureElement};
use crate::machine::model::Carried;
use crate::machine::{KError, KErrorKind};

/// A call's resolved arguments, before binding — the single input to [`KFunction::bind_by_name`].
/// The two forms differ only in how each parameter's value is *located*; binding is identical.
pub enum CallArgs<'a> {
    /// `f a b` — positional values in parameter order. Keywords are the signature's own literals
    /// (matched when the overload was picked), so only the value cells appear here.
    Positional(Vec<Carried<'a>>),
    /// `f {x = a, y = b}` — the named-argument record itself; each parameter is taken by its name.
    /// Leftover names with no matching parameter are dropped (call-by-name width drop, as in
    /// `reconstruct_positional`).
    Named(Record<Carried<'a>>),
}

impl<'a> KFunction<'a> {
    /// Bind `args` to this function's parameters by name, producing the argument record directly.
    pub fn bind_by_name(&'a self, args: CallArgs<'a>) -> Result<Record<Carried<'a>>, KError> {
        bind_args_by_name(&self.signature, args)
    }
}

/// Signature-only core of [`KFunction::bind_by_name`] — needs no body or captured scope, so it is
/// directly testable. Walks the signature's parameters in order; for each, takes its supplied value
/// (by name for [`CallArgs::Named`], by position for [`CallArgs::Positional`]) into the record.
pub fn bind_args_by_name<'a>(
    signature: &ExpressionSignature<'a>,
    args: CallArgs<'a>,
) -> Result<Record<Carried<'a>>, KError> {
    let mut bound: Record<Carried<'a>> = Record::new();
    match args {
        CallArgs::Named(record) => {
            for arg in arguments(signature) {
                let value = *record
                    .get(&arg.name)
                    .ok_or_else(|| KError::new(KErrorKind::MissingArg(arg.name.clone())))?;
                bound.insert(arg.name.clone(), value);
            }
        }
        CallArgs::Positional(values) => {
            let mut values = values.into_iter();
            for arg in arguments(signature) {
                let value = values
                    .next()
                    .ok_or_else(|| KError::new(KErrorKind::MissingArg(arg.name.clone())))?;
                bound.insert(arg.name.clone(), value);
            }
        }
    }
    Ok(bound)
}

/// The signature's parameters in order — `Keyword` elements are the call form's own literals, not
/// bound values, so they are skipped.
fn arguments<'s, 'a>(
    signature: &'s ExpressionSignature<'a>,
) -> impl Iterator<Item = &'s Argument<'a>> {
    signature.elements.iter().filter_map(|el| match el {
        SignatureElement::Argument(arg) => Some(arg),
        SignatureElement::Keyword(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::RuntimeArena;
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
    fn named_and_positional_bind_identically() {
        let arena = RuntimeArena::new();
        let seven = Carried::Object(arena.alloc_object(KObject::Number(7.0)));

        let mut named_args = Record::new();
        named_args.insert("x".to_string(), seven);
        let named = bind_args_by_name(&double_signature(), CallArgs::Named(named_args))
            .expect("named binds");
        assert_eq!(bound_x(&named), 7.0);

        let positional = bind_args_by_name(&double_signature(), CallArgs::Positional(vec![seven]))
            .expect("positional binds");
        assert_eq!(bound_x(&positional), 7.0);
    }

    #[test]
    fn named_missing_parameter_errors() {
        let result = bind_args_by_name(&double_signature(), CallArgs::Named(Record::new()));
        assert!(matches!(
            result,
            Err(e) if matches!(e.kind, KErrorKind::MissingArg(ref n) if n == "x")
        ));
    }
}

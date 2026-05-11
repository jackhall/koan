//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body.
//!
//! Also home to the slot-extraction helpers (`extract_kexpression`, `extract_type_expr`,
//! `extract_bare_type_name`) that collapse the `Rc::try_unwrap` + variant-match dance
//! used to pull `KExpression`, `TypeExpr`, and bare type names out of a bundle slot.

use std::collections::HashMap;
use std::rc::Rc;

use crate::parse::kexpression::{KExpression, TypeExpr, TypeParams};

use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::values::KObject;

/// Name to resolved value, produced by `KFunction::bind` and consumed by the body.
pub struct ArgumentBundle<'a> {
    pub args: HashMap<String, Rc<KObject<'a>>>,
}

impl<'a> ArgumentBundle<'a> {
    pub fn get(&self, name: &str) -> Option<&KObject<'a>> {
        self.args.get(name).map(|v| v.as_ref())
    }

    /// Fully independent copy: each value is `deep_clone`d into a fresh `Rc`. Sharing in
    /// the original bundle's `Rc`s is not preserved.
    pub fn deep_clone(&self) -> ArgumentBundle<'a> {
        ArgumentBundle {
            args: self
                .args
                .iter()
                .map(|(k, v)| (k.clone(), Rc::new(v.deep_clone())))
                .collect(),
        }
    }
}

/// Take ownership of a `KType::KExpression`-typed argument out of `bundle.args`, cloning
/// only if the bundle is not the sole `Rc` holder. Returns `None` if the slot is missing
/// or holds a non-`KExpression` variant.
pub(crate) fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

/// Take ownership of the structured `TypeExpr` carried by a `KType::TypeExprRef` slot.
/// Resolve preserves the parser's `TypeExpr` as `KObject::TypeExprValue` so parameterized
/// types (`List<Number>`, `Function<(N) -> S>`) survive into the builtin's body intact.
pub(crate) fn extract_type_expr<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<TypeExpr> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::TypeExprValue(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::TypeExprValue(t) => Some(t.clone()),
            _ => None,
        },
    }
}

/// Resolve a `KType::TypeExprRef` slot to its bare type name, rejecting parameterized
/// forms (`Foo<X>`). `surface` is the surface-form keyword (`"STRUCT"`, `"UNION"`, ...)
/// embedded in the `ShapeError` message.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::TypeExprValue(t)) => match &t.params {
            TypeParams::None => Ok(t.name.clone()),
            _ => Err(KError::new(KErrorKind::ShapeError(format!(
                "{surface} {name} must be a bare type name, got `{}`",
                t.render(),
            )))),
        },
        Some(other) => Err(KError::new(KErrorKind::TypeMismatch {
            arg: name.to_string(),
            expected: "TypeExprRef".to_string(),
            got: other.ktype().name(),
        })),
        None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
    }
}

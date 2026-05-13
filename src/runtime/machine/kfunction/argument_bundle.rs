//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body.
//!
//! Also home to the slot-extraction helpers (`extract_kexpression`, `extract_type_expr`,
//! `extract_bare_type_name`) that collapse the `Rc::try_unwrap` + variant-match dance
//! used to pull `KExpression`, `TypeExpr`, and bare type names out of a bundle slot.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::KExpression;

use crate::runtime::machine::core::{KError, KErrorKind};
use crate::runtime::model::types::KType;
use crate::runtime::model::values::KObject;

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

/// Take ownership of the elaborated `KType` carried by a `KType::TypeExprRef` slot.
/// Bindings now store `KObject::KTypeValue(kt)` directly; this helper pulls the inner
/// `KType` out, cloning if the bundle is not the sole `Rc` holder.
pub(crate) fn extract_ktype<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KType> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::KTypeValue(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KTypeValue(t) => Some(t.clone()),
            _ => None,
        },
    }
}

/// Resolve a `KType::TypeExprRef` slot to its bare type name. After the
/// `KObject::TypeExprValue → KObject::KTypeValue` migration, the slot's payload is an
/// elaborated `KType`; only leaf-named variants are valid binder / constructor / type-call
/// names. Parameterized forms (`List<X>`), function types, `Mu` / `RecursiveRef`, and the
/// other structural variants are rejected as `ShapeError`. `surface` is the surface-form
/// keyword (`"STRUCT"`, `"UNION"`, …) embedded in the message.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::KTypeValue(t)) => match t {
            // Leaf-named variants: surface name is the user-facing identifier.
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Identifier
            | KType::KExpression
            | KType::TypeExprRef
            | KType::Type
            | KType::Tagged
            | KType::Struct
            | KType::Module
            | KType::Signature
            | KType::Any
            | KType::ModuleType { .. }
            | KType::SignatureBound { .. }
            | KType::Unresolved(_) => Ok(t.name()),
            // Structural / recursive shapes are not valid binder names — the caller wants
            // a leaf identifier, not a parameterized container.
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => Err(KError::new(KErrorKind::ShapeError(format!(
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

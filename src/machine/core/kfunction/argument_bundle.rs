//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body.

use std::rc::Rc;

use crate::machine::model::ast::{KExpression, TypeName};

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::types::{KType, Record};
use crate::machine::model::values::{KObject, Module, Signature};

pub struct ArgumentBundle<'a> {
    pub args: Record<Rc<KObject<'a>>>,
}

impl<'a> ArgumentBundle<'a> {
    pub fn get(&self, name: &str) -> Option<&KObject<'a>> {
        self.args.get(name).map(|v| v.as_ref())
    }

    /// Independent copy; `Rc` sharing in the original is not preserved.
    pub fn deep_clone(&self) -> ArgumentBundle<'a> {
        ArgumentBundle {
            args: Record::from_pairs(
                self.args
                    .iter()
                    .map(|(k, v)| (k.clone(), Rc::new(v.deep_clone()))),
            ),
        }
    }

    pub fn require_kexpression(&self, name: &str) -> Result<&KExpression<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_kexpression()
            .ok_or_else(|| mismatch(name, "KExpression", obj))
    }

    pub fn require_ktype(&self, name: &str) -> Result<&KType<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_ktype()
            .ok_or_else(|| mismatch(name, "TypeExprRef", obj))
    }

    pub fn require_module(&self, name: &str) -> Result<&'a Module<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_module().ok_or_else(|| mismatch(name, "Module", obj))
    }

    pub fn require_signature(&self, name: &str) -> Result<&'a Signature<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_signature()
            .ok_or_else(|| mismatch(name, "Signature", obj))
    }

    /// Untyped variant of the `require_*` family; caller dispatches on `KObject` arms.
    pub fn require(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get_or_missing(name)
    }

    fn get_or_missing(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get(name)
            .ok_or_else(|| KError::new(KErrorKind::MissingArg(name.to_string())))
    }
}

fn mismatch(arg: &str, expected: &str, got: &KObject<'_>) -> KError {
    KError::new(KErrorKind::TypeMismatch {
        arg: arg.to_string(),
        expected: expected.to_string(),
        got: got.ktype().name(),
    })
}

/// Move the `KExpression` out of `bundle.args`, cloning only if the `Rc` is shared.
/// `None` for missing slot or non-`KExpression` variant.
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

/// Move a `KObject::KTypeValue` out of `bundle.args`, cloning only if the `Rc` is shared.
/// `None` for the sibling `KObject::TypeNameRef` carrier ([`extract_type_name_ref`]) and
/// for missing slots.
///
/// Both extractors consume the slot via `remove`; callers that try both must peek with
/// `bundle.get(...)` first.
pub(crate) fn extract_ktype<'a>(bundle: &mut ArgumentBundle<'a>, name: &str) -> Option<KType<'a>> {
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

/// Resolve a `KType::TypeExprRef` slot to its bare type name. Either carrier may occupy
/// the slot: a `KTypeValue` (parser `TypeName` resolved to a builtin) or a `TypeNameRef`
/// (unresolved-leaf fallback). Structural / parameterized shapes from either carrier are
/// rejected as `ShapeError`. `surface` is the keyword (`"STRUCT"`, `"UNION"`, …) embedded
/// in the message.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::TypeNameRef(t)) => Ok(t.render()),
        Some(KObject::KTypeValue(t)) => match t {
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Identifier
            | KType::KExpression
            | KType::SigiledTypeExpr
            | KType::TypeExprRef
            | KType::Type
            | KType::AnyModule
            | KType::AnySignature
            | KType::Any
            | KType::SetRef { .. }
            | KType::AnyUserType { .. }
            | KType::Signature { .. }
            | KType::Module { .. }
            | KType::AbstractType { .. } => Ok(t.name()),
            KType::List(_)
            | KType::Dict(_, _)
            | KType::Record(_)
            | KType::KFunction { .. }
            | KType::KFunctor { .. }
            | KType::DeferredReturn(_)
            | KType::SetLocal(_)
            | KType::RecursiveRef(_)
            | KType::RecursiveGroup(_)
            | KType::ConstructorApply { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
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

/// Move a `TypeNameRef` carrier's `TypeName` out of `bundle.args`, cloning only if the
/// `Rc` is shared. `None` for missing slot or non-`TypeNameRef` variant — pair with
/// [`extract_ktype`] for the resolved-leaf fallthrough.
pub(crate) fn extract_type_name_ref<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<TypeName> {
    let rc = bundle.args.remove(name)?;
    match Rc::try_unwrap(rc) {
        Ok(KObject::TypeNameRef(t)) => Some(t),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::TypeNameRef(t) => Some(t.clone()),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests;

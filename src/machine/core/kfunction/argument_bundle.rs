//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body.
//!
//! Also home to the slot-extraction helpers (`extract_kexpression`, `extract_ktype`,
//! `extract_type_name_ref`, `extract_bare_type_name`) that collapse the
//! `Rc::try_unwrap` + variant-match dance used to pull `KExpression`, an elaborated
//! `KType`, a `TypeNameRef` carrier's `TypeExpr`, or a surface type name out of a
//! bundle slot.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::model::ast::{KExpression, TypeExpr, TypeParams};

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::types::KType;
use crate::machine::model::values::{KObject, Module, Signature};

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

    /// Borrow `name`'s slot as a `&KExpression`. `MissingArg` if absent;
    /// `TypeMismatch { expected: "KExpression" }` if the slot holds a non-`KExpression`
    /// variant.
    pub fn require_kexpression(&self, name: &str) -> Result<&KExpression<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_kexpression().ok_or_else(|| mismatch(name, "KExpression", obj))
    }

    /// Borrow `name`'s slot as a `&KType`. `MissingArg` / `TypeMismatch` shaped the same
    /// way as [`require_kexpression`](Self::require_kexpression).
    pub fn require_ktype(&self, name: &str) -> Result<&KType, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_ktype().ok_or_else(|| mismatch(name, "TypeExprRef", obj))
    }

    /// Borrow `name`'s slot as a `&Module`. Same error shape as the sister `require_*`
    /// methods.
    pub fn require_module(&self, name: &str) -> Result<&'a Module<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_module().ok_or_else(|| mismatch(name, "Module", obj))
    }

    /// Borrow `name`'s slot as a `&Signature`. Same error shape as the sister `require_*`
    /// methods.
    pub fn require_signature(&self, name: &str) -> Result<&'a Signature<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_signature().ok_or_else(|| mismatch(name, "Signature", obj))
    }

    /// Borrow `name`'s slot as any `&KObject`. `MissingArg` if absent; no variant
    /// narrowing — the caller dispatches on `KObject` arms itself. Use the variant-typed
    /// `require_*` siblings when only one shape is acceptable.
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

/// Take ownership of the elaborated `KType` carried by a `KObject::KTypeValue`-variant
/// `KType::TypeExprRef` slot. Returns `None` for the sibling `KObject::TypeNameRef`
/// carrier (callers route to [`extract_type_name_ref`] for that path) and for missing
/// slots. Clones the inner `KType` if the bundle is not the sole `Rc` holder.
///
/// Both extractors consume the slot via `remove`; a caller that wants to try both must
/// peek with `bundle.get(...)` first to pick the right one.
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

/// Resolve a `KType::TypeExprRef` slot to its bare type name. Two carrier variants share
/// the slot post-stage-2:
///
/// - `KObject::KTypeValue(t)` — the parser-side `TypeExpr` resolved to a builtin `KType`
///   at `resolve_for` time. Leaf-named variants surface their `KType::name()`;
///   structural / recursive shapes (`List<X>`, function types, `Mu` / `RecursiveRef`)
///   are not valid binder / constructor / type-call names and surface a `ShapeError`.
/// - `KObject::TypeNameRef(t, _)` — a `resolve_for`-time fallback for bare-leaf names
///   not in `KType::from_name`'s builtin table. The surface name is `t.name` directly;
///   parameterized shapes on the carrier's `TypeExpr` are rejected with the same
///   `ShapeError` text shape as the parameterized-`KType` rejection.
///
/// `surface` is the surface-form keyword (`"STRUCT"`, `"UNION"`, …) embedded in the
/// message.
pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get(name) {
        Some(KObject::TypeNameRef(t)) => match &t.params {
            TypeParams::None => Ok(t.name.clone()),
            // Parameterized surface form on a `TypeNameRef` carrier — the parser saw
            // something like `Foo<Bar>` where `Foo` isn't a builtin and the user wrote
            // it in a binder / constructor slot. Reject with the same message shape as
            // the `KTypeValue` parameterized rejection.
            TypeParams::List(_) | TypeParams::Function { .. } => {
                Err(KError::new(KErrorKind::ShapeError(format!(
                    "{surface} {name} must be a bare type name, got `{}`",
                    t.render(),
                ))))
            }
        },
        Some(KObject::KTypeValue(t)) => match t {
            // Leaf-named variants: surface name is the user-facing identifier. Both
            // `UserType` (per-declaration tag) and `AnyUserType` (wildcard kind tag)
            // join the leaf set — their `name()` renders either the declared name or
            // the surface keyword (`Struct`/`Tagged`/`Module`), both valid binder /
            // constructor / type-call names.
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Identifier
            | KType::KExpression
            | KType::TypeExprRef
            | KType::Type
            | KType::Signature
            | KType::Any
            | KType::UserType { .. }
            | KType::AnyUserType { .. }
            | KType::SignatureBound { .. } => Ok(t.name()),
            // Structural / recursive shapes are not valid binder names — the caller wants
            // a leaf identifier, not a parameterized container. `ConstructorApply` joins
            // this group: an applied higher-kinded type is structural, not a leaf.
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_)
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

/// Take ownership of a `TypeNameRef` carrier's `TypeExpr` out of `bundle.args`, cloning
/// if the bundle is not the sole `Rc` holder. Returns `None` when the slot is missing or
/// holds a non-`TypeNameRef` variant (the caller typically tried `extract_ktype` first
/// and falls through here for the unresolved-leaf carrier path).
///
/// FN's return-type elaboration consumes the helper to recover the bare-leaf name into
/// its existing `ReturnTypeState::Pending(name, …)` / `ReturnTypeCapture::Unresolved`
/// machinery; the parser-preserved `TypeExpr` is the source of truth for the surface
/// form that survives bind for diagnostics.
pub(crate) fn extract_type_name_ref<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<TypeExpr> {
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

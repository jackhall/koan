//! `ArgumentBundle` — the resolved name-to-value map produced by `KFunction::bind` and
//! consumed by a builtin or user-defined body. Each argument is an [`ArgValue`]: a runtime
//! object (`Object` arm) or a type flowing in the type channel (`Type` arm).

use crate::machine::model::ast::KExpression;

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::types::{KType, Record};
use crate::machine::model::values::{ArgValue, KObject, Module, Signature};

pub struct ArgumentBundle<'a> {
    pub args: Record<ArgValue<'a>>,
}

impl<'a> ArgumentBundle<'a> {
    /// The `Object` arm of `name`, if present and a runtime object.
    pub fn get(&self, name: &str) -> Option<&KObject<'a>> {
        self.args.get(name).and_then(|v| v.as_object())
    }

    /// The `Type` arm of `name`, if present and a type.
    pub fn get_type(&self, name: &str) -> Option<&KType<'a>> {
        self.args.get(name).and_then(|v| v.as_type())
    }

    /// Independent copy; `Rc` sharing in the original is not preserved.
    pub fn deep_clone(&self) -> ArgumentBundle<'a> {
        ArgumentBundle {
            args: Record::from_pairs(self.args.iter().map(|(k, v)| (k.clone(), v.deep_clone()))),
        }
    }

    pub fn require_kexpression(&self, name: &str) -> Result<&KExpression<'a>, KError> {
        let obj = self.get_or_missing(name)?;
        obj.as_kexpression()
            .ok_or_else(|| mismatch(name, "KExpression", obj))
    }

    /// The `Type`-arm type of `name`. Errors if the slot is missing or holds an object.
    pub fn require_ktype(&self, name: &str) -> Result<&KType<'a>, KError> {
        match self.args.get(name) {
            Some(ArgValue::Type(kt)) => Ok(kt),
            Some(ArgValue::Object(rc)) => Err(mismatch(name, "TypeExprRef", rc)),
            None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
        }
    }

    pub fn require_module(&self, name: &str) -> Result<&'a Module<'a>, KError> {
        match self.get_type(name) {
            Some(KType::Module { module, .. }) => Ok(module),
            _ => Err(self.type_mismatch_or_missing(name, "Module")),
        }
    }

    pub fn require_signature(&self, name: &str) -> Result<&'a Signature<'a>, KError> {
        match self.get_type(name) {
            Some(KType::Signature { sig, .. }) => Ok(sig),
            _ => Err(self.type_mismatch_or_missing(name, "Signature")),
        }
    }

    /// Untyped variant of the `require_*` family; caller dispatches on `KObject` arms.
    pub fn require(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get_or_missing(name)
    }

    /// Move the `KExpression` out of slot `name`, or produce the canonical
    /// parenthesized-slot `ShapeError` (`"<BUILTIN> <slot> slot must be a
    /// parenthesized expression"`) when the slot is missing or non-`KExpression`.
    /// The single owner of that error text — builtins call this instead of
    /// open-coding the `match extract_kexpression { … None => err(…) }` envelope.
    pub(crate) fn extract_kexpression_or_shape_error(
        &mut self,
        builtin: &str,
        slot: &str,
    ) -> Result<KExpression<'a>, KError> {
        extract_kexpression(self, slot).ok_or_else(|| {
            KError::new(KErrorKind::ShapeError(format!(
                "{builtin} {slot} slot must be a parenthesized expression"
            )))
        })
    }

    fn get_or_missing(&self, name: &str) -> Result<&KObject<'a>, KError> {
        self.get(name)
            .ok_or_else(|| KError::new(KErrorKind::MissingArg(name.to_string())))
    }

    fn type_mismatch_or_missing(&self, name: &str, expected: &str) -> KError {
        match self.args.get(name) {
            Some(v) => KError::new(KErrorKind::TypeMismatch {
                arg: name.to_string(),
                expected: expected.to_string(),
                got: arg_value_type_name(v),
            }),
            None => KError::new(KErrorKind::MissingArg(name.to_string())),
        }
    }
}

fn arg_value_type_name(v: &ArgValue<'_>) -> String {
    match v {
        ArgValue::Object(o) => o.ktype().name(),
        ArgValue::Type(kt) => kt.name(),
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
/// `None` for missing slot or non-`KExpression` (the slot is an object whose `Rc` holds a
/// `KExpression`).
pub(crate) fn extract_kexpression<'a>(
    bundle: &mut ArgumentBundle<'a>,
    name: &str,
) -> Option<KExpression<'a>> {
    let rc = match bundle.args.remove(name)? {
        ArgValue::Object(rc) => rc,
        ArgValue::Type(_) => return None,
    };
    match std::rc::Rc::try_unwrap(rc) {
        Ok(KObject::KExpression(e)) => Some(e),
        Ok(_) => None,
        Err(rc) => match &*rc {
            KObject::KExpression(e) => Some(e.clone()),
            _ => None,
        },
    }
}

/// Move the `Type`-arm `KType` out of `bundle.args`. Returns any type — a resolved `KType` or
/// the [`KType::Unresolved`] transient for a bare user name; callers branch on `Unresolved`.
/// `None` for a missing slot or an object slot.
pub(crate) fn extract_ktype<'a>(bundle: &mut ArgumentBundle<'a>, name: &str) -> Option<KType<'a>> {
    match bundle.args.remove(name)? {
        ArgValue::Type(kt) => Some(kt),
        ArgValue::Object(_) => None,
    }
}

/// Resolve a type-name slot to its bare type name. A bare-leaf / wildcard / nominal type
/// renders directly; structural / parameterized shapes are rejected as `ShapeError`.
/// `surface` is the keyword (`"STRUCT"`, `"UNION"`, …) embedded in the message.
/// The bare-name check on a resolved `KType` (shared by [`extract_bare_type_name`] and the
/// `Action`-harness binders that read their name from a `KObject::Record` type cell): a simple /
/// nominal leaf yields its `name()`; a structural type (List, Record, FN, …) is a shape error.
pub(crate) fn bare_type_name<'a>(t: &KType<'a>, name: &str, surface: &str) -> Result<String, KError> {
    match t {
        KType::Number
        | KType::Str
        | KType::Bool
        | KType::Null
        | KType::Identifier
        | KType::KExpression
        | KType::SigiledTypeExpr
        | KType::RecordType
        | KType::OfKind(_)
        | KType::Unresolved(_)
        | KType::Any
        | KType::SetRef { .. }
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
        | KType::Variant { .. }
        | KType::RecursiveRef(_)
        | KType::RecursiveGroup(_)
        | KType::ConstructorApply { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
            "{surface} {name} must be a bare type name, got `{}`",
            t.render(),
        )))),
    }
}

pub(crate) fn extract_bare_type_name<'a>(
    bundle: &ArgumentBundle<'a>,
    name: &str,
    surface: &str,
) -> Result<String, KError> {
    match bundle.get_type(name) {
        Some(t) => bare_type_name(t, name, surface),
        None => match bundle.args.get(name) {
            Some(_) => Err(KError::new(KErrorKind::TypeMismatch {
                arg: name.to_string(),
                expected: "TypeExprRef".to_string(),
                got: bundle
                    .get(name)
                    .map(|o| o.ktype().name())
                    .unwrap_or_default(),
            })),
            None => Err(KError::new(KErrorKind::MissingArg(name.to_string()))),
        },
    }
}

#[cfg(test)]
mod tests;

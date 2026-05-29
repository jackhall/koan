//! Struct construction. Parallels [`tagged_union`](super::tagged_union) but is
//! variable-arity and named-only: `Point (x: 3, y: 4)` parses as
//! [`NamedPairs`](crate::machine::model::values::NamedPairs), reorders into
//! schema declaration order, and re-dispatches through the construction
//! primitive registered below.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::core::{KError, KErrorKind, Scope, ScopeId};
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement, UserTypeKind,
};
use crate::machine::model::values::{KObject, NamedPairs};

use super::register_builtin;

/// Error precedence is missing-field before unknown-field: telling the user
/// "you forgot `y`" is more actionable than "you have a stray `z`".
/// Wrapping each value-part in a single-part sub-expression gives identifiers
/// and literals uniform routing through the scheduler's normal dispatch lanes.
pub fn apply<'a>(
    schema_obj: &'a KObject<'a>,
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> BodyResult<'a> {
    let fields = match schema_obj.as_struct_type() {
        Some((_, fields)) => Rc::clone(fields),
        None => {
            debug_assert!(false, "struct_value::apply called on non-StructType");
            return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                "struct_value::apply called on non-StructType".to_string(),
            )));
        }
    };
    let tmp_expr = KExpression::new(args_parts);
    let mut pairs = match NamedPairs::parse(&tmp_expr, "struct construction") {
        Ok(p) => p,
        Err(msg) => return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg))),
    };
    let mut wrapped: Vec<ExpressionPart<'a>> = Vec::with_capacity(fields.len());
    for (field_name, _) in fields.iter() {
        match pairs.take(field_name) {
            Some(v) => wrapped.push(ExpressionPart::expression(vec![v])),
            None => {
                return BodyResult::Err(KError::new(KErrorKind::MissingArg(field_name.clone())));
            }
        }
    }
    if let Some(unknown) = pairs.into_unknown() {
        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
            "unknown field `{unknown}` in struct construction",
        ))));
    }
    let parts = vec![
        Spanned::bare(ExpressionPart::Future(schema_obj)),
        Spanned::bare(ExpressionPart::ListLiteral(wrapped)),
    ];
    BodyResult::tail(KExpression::new(parts))
}

/// Pure construction step: validates length and per-position types, then
/// emits the `KObject::Struct`. No scope, no scheduler.
pub fn construct<'a>(
    type_name: &str,
    scope_id: ScopeId,
    fields: &[(String, KType<'a>)],
    values: &[KObject<'a>],
) -> Result<KObject<'a>, KError> {
    if values.len() != fields.len() {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: values.len(),
        }));
    }
    // IndexMap: declaration-order iteration so PRINT / summarize match the schema.
    let mut map: IndexMap<String, KObject<'a>> = IndexMap::with_capacity(fields.len());
    for ((field_name, expected), value) in fields.iter().zip(values.iter()) {
        if !expected.matches_value(value) {
            return Err(KError::new(KErrorKind::TypeMismatch {
                arg: field_name.clone(),
                expected: expected.name().to_string(),
                got: value.ktype().name().to_string(),
            }));
        }
        map.insert(field_name.clone(), value.deep_clone());
    }
    // Copy `(scope_id, name)` off the schema so the value carries the
    // declaring schema's identity (read by `ktype()`).
    Ok(KObject::Struct {
        name: type_name.to_string(),
        scope_id,
        fields: Rc::new(map),
    })
}

/// Shim around [`construct`]: unpacks `schema` and `values` from the bundle
/// and arena-allocates the result.
fn primitive_body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (type_name, scope_id, fields) = match bundle.get("schema") {
        Some(KObject::StructType { name, scope_id, fields }) => {
            (name.clone(), *scope_id, Rc::clone(fields))
        }
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "schema".to_string(),
                expected: "StructType".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("schema".to_string())));
        }
    };
    let values = match bundle.get("values") {
        Some(KObject::List(items, _)) => Rc::clone(items),
        Some(other) => {
            return BodyResult::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "values".to_string(),
                expected: "List".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => {
            return BodyResult::Err(KError::new(KErrorKind::MissingArg("values".to_string())));
        }
    };
    match construct(&type_name, scope_id, &fields, &values) {
        Ok(struct_value) => BodyResult::Value(scope.arena.alloc(struct_value)),
        Err(e) => BodyResult::Err(e),
    }
}

/// Signature is `Type, List` — no keyword. The union construct primitive is
/// 3-slot, so the `[Slot, Slot]` bucket is unambiguous between the two.
pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "struct_construct",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::AnyUserType { kind: UserTypeKind::Struct }),
            elements: vec![
                SignatureElement::Argument(Argument { name: "schema".into(), ktype: KType::Type }),
                SignatureElement::Argument(Argument { name: "values".into(), ktype: KType::List(Box::new(KType::Any)) }),
            ],
        },
        primitive_body,
    );
}

#[cfg(test)]
mod tests;

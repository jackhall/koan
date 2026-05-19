//! Struct construction primitives, paralleling [`tagged_union`](super::tagged_union) for
//! product types. `apply` is the entry point both surface forms (type-token call via
//! [`type_call`](super::type_call) and identifier-bound type call via
//! [`call_by_name`](super::call_by_name)) call. It synthesizes a tail expression
//! that re-dispatches through the construction-primitive builtin defined here.
//!
//! Unlike the tagged-union primitive (3 fixed slots: schema/tag/value), struct construction
//! is variable-arity — a `Point` schema declares 2 fields, a `User` schema might declare 5.
//! Construction is **named-only**: the user writes `Point (x: 3, y: 4)` and `apply` parses
//! the inner expression as `<name>: <value>` triples (via
//! [`NamedPairs`](crate::machine::model::values::NamedPairs)) and consumes one
//! value per declared field in schema-declaration order. Reordered value-parts are then wrapped in single-part sub-expressions
//! inside a `ListLiteral`. The scheduler aggregates the list, dispatching each wrapped
//! sub-expression through `value_lookup`/`value_pass` so identifiers and literals both
//! resolve to their values before the primitive sees the assembled `KObject::List`. The
//! primitive then validates per-field types against the schema and emits a `KObject::Struct`.

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

/// Parse the inner expression of a `Point (x: 3, y: 4)` form as named pairs, validate the
/// names match the schema, reorder the values into schema declaration order, and synthesize
/// a tail that re-dispatches through the construction primitive.
///
/// Validation precedence (first wins): missing field → unknown field. Missing-first
/// because telling the user "you forgot `y`" is more actionable than "you have a stray
/// `z`" — adding the missing field is what they need either way. Arity is implicit:
/// [`NamedPairs`](crate::machine::model::values::NamedPairs) rejects duplicate
/// names at parse time, so once every declared field has been taken and the residual is
/// empty, the input matched the schema exactly.
///
/// After reordering, each value-part is wrapped in a single-part sub-expression so bare
/// identifiers route through `value_lookup` and bare literals through `value_pass` —
/// uniform handling regardless of surface form. The wrapped parts are bundled into an
/// `ExpressionPart::ListLiteral`, which the scheduler aggregates into a `KObject::List`
/// before the construction primitive runs.
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
    // Missing-first error precedence: each declared field consumes its named value, so
    // a missing-field error fires before any unknown-field surfacing.
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

/// Validate `values` against `fields` (matching length and per-position types) and build
/// the `KObject::Struct`. Pure logic — no scope, no scheduler. The construction-primitive
/// builtin's body is a thin shim around this.
pub fn construct<'a>(
    type_name: &str,
    scope_id: ScopeId,
    fields: &[(String, KType)],
    values: &[KObject<'a>],
) -> Result<KObject<'a>, KError> {
    if values.len() != fields.len() {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: values.len(),
        }));
    }
    // Insert in declaration order so iteration (via PRINT / summarize) matches the schema.
    // IndexMap preserves insertion order while keeping O(1) keyed lookup.
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
    // Stage 3.0c: copy `(scope_id, name)` off the schema's `StructType` so the value
    // carries the declaring schema's identity. Stage 3.1 reads these in `ktype()`.
    Ok(KObject::Struct {
        name: type_name.to_string(),
        scope_id,
        fields: Rc::new(map),
    })
}

/// Body of the construction-primitive builtin. Pulls the struct schema and the assembled
/// values list out of the bundle, calls [`construct`], and arena-allocates the result.
fn primitive_body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Pull `(scope_id, name)` off the schema so the produced `Struct` value carries
    // the declaring schema's identity — stage 3.0c made this load-bearing for 3.1's
    // `ktype()` flip.
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
        Some(KObject::List(items)) => Rc::clone(items),
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
        Ok(struct_value) => BodyResult::Value(scope.arena.alloc_object(struct_value)),
        Err(e) => BodyResult::Err(e),
    }
}

/// Register the struct construction primitive. No keyword in the signature — slot 0 is
/// `Type` (matches both `StructType` and `TaggedUnionType`, but the union construct
/// primitive is 3-slot so the bucket keys differ) and slot 1 is `List`. The `[Slot, Slot]`
/// bucket is shared with other 2-arg signatures; specificity ranks our `Type+List` slots
/// above more permissive ones.
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

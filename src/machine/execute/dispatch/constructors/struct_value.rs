//! Struct construction. `prepare_value_parts` reorders the call-site
//! record fields into schema declaration order; `construct` validates
//! each value against its field type and emits the `KObject::Struct`.
//! See [`constructors::dispatch_construct`](super::dispatch_construct)
//! for the dispatch entry.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::machine::core::{KError, KErrorKind, ScopeId};
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{KType, Record};
use crate::machine::model::values::{KObject, NamedPairs};

/// Reorder call-site args into schema declaration order. Error
/// precedence is missing-field before unknown-field: telling the user
/// "you forgot `y`" is more actionable than "you have a stray `z`".
pub(in crate::machine::execute) fn prepare_value_parts<'a>(
    fields: &Rc<Record<KType<'a>>>,
    record_fields: Vec<(String, ExpressionPart<'a>)>,
) -> Result<Vec<ExpressionPart<'a>>, KError> {
    let mut pairs = NamedPairs::from_fields(record_fields)
        .map_err(|msg| KError::new(KErrorKind::ShapeError(msg)))?;
    let mut ordered: Vec<ExpressionPart<'a>> = Vec::with_capacity(fields.len());
    for (field_name, _) in fields.iter() {
        match pairs.take(field_name) {
            Some(v) => ordered.push(v),
            None => return Err(KError::new(KErrorKind::MissingArg(field_name.clone()))),
        }
    }
    if let Some(unknown) = pairs.into_unknown() {
        return Err(KError::new(KErrorKind::ShapeError(format!(
            "unknown field `{unknown}` in struct construction",
        ))));
    }
    Ok(ordered)
}

/// Validate length and per-position types, then emit the
/// `KObject::Struct`. `values` is in schema declaration order — match
/// the output of [`prepare_value_parts`].
pub(in crate::machine::execute) fn construct<'a>(
    type_name: &str,
    scope_id: ScopeId,
    fields: &Record<KType<'a>>,
    values: &[&'a KObject<'a>],
) -> Result<KObject<'a>, KError> {
    if values.len() != fields.len() {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: fields.len(),
            got: values.len(),
        }));
    }
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
    Ok(KObject::Struct {
        name: type_name.to_string(),
        scope_id,
        fields: Rc::new(map),
    })
}

#[cfg(test)]
mod tests;

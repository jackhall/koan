//! Tagged-union construction. `prepare_args` validates the call-site
//! `(tag value)` shape; `construct` checks the tag against the schema
//! and the value against the tag's expected type, then emits the
//! `KObject::Tagged`. See
//! [`constructors::dispatch_construct`](super::dispatch_construct) for
//! the dispatch entry.

use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::source::Spanned;
use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{KType, RecursiveSet};
use crate::machine::model::values::KObject;

/// Validate the args shape: exactly two parts, the first an `Identifier`
/// tag. The value part rides through unchanged so the dispatcher can
/// sub-Dispatch it before construction sees its resolved value.
pub(in crate::machine::execute) fn prepare_args<'a>(
    args_parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> Result<(String, ExpressionPart<'a>), KError> {
    if args_parts.len() != 2 {
        return Err(KError::new(KErrorKind::ArityMismatch {
            expected: 2,
            got: args_parts.len(),
        }));
    }
    let mut iter = args_parts.into_iter();
    let tag_part = iter.next().unwrap();
    let value_part = iter.next().unwrap();
    let tag = match tag_part.value {
        ExpressionPart::Identifier(s) => s,
        other => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "tagged-union construction = first arg must be a bare-identifier tag, got {}",
                other.summarize()
            ))));
        }
    };
    Ok((tag, value_part.value))
}

/// Validate `tag` against `schema` and `value` against the schema's
/// expected type for that tag, then emit the `KObject::Tagged`.
pub(in crate::machine::execute) fn construct<'a>(
    schema: &HashMap<String, KType<'a>>,
    set: &Rc<RecursiveSet<'a>>,
    index: usize,
    tag: String,
    value: &KObject<'a>,
) -> Result<KObject<'a>, KError> {
    let expected = match schema.get(&tag) {
        Some(t) => t.clone(),
        None => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "tag `{}` not in union (known: {})",
                tag,
                schema.keys().cloned().collect::<Vec<_>>().join(", ")
            ))));
        }
    };
    if !expected.matches_value(value) {
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "value".to_string(),
            expected: expected.name().to_string(),
            got: value.ktype().name().to_string(),
        }));
    }
    Ok(KObject::Tagged {
        tag,
        value: Rc::new(value.deep_clone()),
        set: Rc::clone(set),
        index,
        type_args: Rc::new(vec![]),
    })
}

#[cfg(test)]
mod tests;

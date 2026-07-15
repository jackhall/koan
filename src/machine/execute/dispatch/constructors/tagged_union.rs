//! Tagged-union construction. `prepare_args` validates the call-site
//! `(tag value)` shape; the tag/value-type checks and the witnessed
//! `KObject::Tagged` build live in
//! [`constructors::finish_witnessed`](super::finish_witnessed), which folds the
//! value carrier's reach onto the result. See
//! [`constructors::dispatch_construct`](super::dispatch_construct) for
//! the dispatch entry.

use crate::machine::core::{KError, KErrorKind};
use crate::machine::model::ExpressionPart;
use crate::source::Spanned;

/// Validate the args shape: exactly two parts, the first a `Type`-token
/// tag (tags are capitalized variant types). The value part rides through
/// unchanged so the dispatcher can sub-Dispatch it before construction
/// sees its resolved value.
pub(in crate::machine::execute) fn prepare_args<'step>(
    args_parts: Vec<Spanned<ExpressionPart<'step>>>,
) -> Result<(String, ExpressionPart<'step>), KError> {
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
        ExpressionPart::Type(t) => t.render(),
        other => {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "tagged-union construction = first arg must be a capitalized variant tag, got {}",
                other.summarize()
            ))));
        }
    };
    Ok((tag, value_part.value))
}

#[cfg(test)]
mod tests;

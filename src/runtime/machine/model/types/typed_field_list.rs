//! Shared parser for `(<name>: <Type> <name>: <Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::{elaborate_type_expr, ElabResult, Elaborator};
use crate::ast::{ExpressionPart, KExpression};
use crate::parse::parse_triple_list;
use crate::runtime::machine::NodeId;

/// Result of one walk over a schema's field list.
pub enum FieldListOutcome {
    /// Every field type elaborated against the captured scope.
    Done(Vec<(String, KType)>),
    /// One or more field-type leaf names parked on outstanding type-binding placeholders.
    /// The caller schedules a Combine over `producers` and re-runs `parse_typed_field_list_via_elaborator`
    /// in the finish closure.
    Park(Vec<NodeId>),
    /// Structural / unbound / cycle error.
    Err(String),
}

/// Phase-3 entry point used by STRUCT / UNION. Routes each field type through the
/// scheduler-aware [`elaborate_type_expr`], accumulating any parking producers across the
/// whole field-list walk so the caller can install one Combine for the merged list.
pub fn parse_typed_field_list_via_elaborator(
    expr: &KExpression<'_>,
    context: &str,
    elaborator: &mut Elaborator<'_, '_>,
) -> FieldListOutcome {
    let mut parks: Vec<NodeId> = Vec::new();
    let parsed = parse_triple_list(expr, context, |part, name| match part {
        ExpressionPart::Type(t) => match elaborate_type_expr(elaborator, t) {
            ElabResult::Done(kt) => Ok(kt),
            ElabResult::Park(producers) => {
                parks.extend(producers);
                // Placeholder KType — discarded when the caller routes through the Park
                // outcome. Keeps the parse_triple_list walk going so we accumulate every
                // parking producer in one pass.
                Ok(KType::Any)
            }
            ElabResult::Unbound(msg) => Err(format!("{msg} in {context} for `{}`", name)),
        },
        other => Err(format!(
            "{context} type for `{}` must be a type name token, got {}",
            name,
            other.summarize()
        )),
    });
    match parsed {
        Err(msg) => FieldListOutcome::Err(msg),
        Ok(fields) => {
            if !parks.is_empty() {
                FieldListOutcome::Park(parks)
            } else {
                FieldListOutcome::Done(fields)
            }
        }
    }
}


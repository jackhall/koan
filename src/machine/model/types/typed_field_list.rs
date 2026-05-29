//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::model::Parseable;
use crate::parse::parse_pair_list;
use crate::machine::NodeId;

pub enum FieldListOutcome<'a> {
    Done(Vec<(String, KType<'a>)>),
    /// `sub_dispatches` carries `(slot_idx_in_schema_parts, wrapped_expression)`
    /// so the caller can splice each resolved `KObject::KTypeValue` back into the
    /// right slot before re-walking.
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
    Err(String),
}

/// Entry point used by STRUCT / UNION. Routes each field type through the
/// scheduler-aware [`elaborate_type_expr`], accumulating parking producers and
/// pending sub-Dispatches across the whole walk so the caller can install one
/// Combine for the merged set.
pub fn parse_typed_field_list_via_elaborator<'a>(
    expr: &KExpression<'a>,
    context: &str,
    elaborator: &mut Elaborator<'_, 'a>,
) -> FieldListOutcome<'a> {
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<(usize, KExpression<'a>)> = Vec::new();
    // `parse_pair_list` walks `[name, slot, name, slot, ...]`; slot index is `2*pair_idx + 1`.
    let mut pair_idx: usize = 0;
    let parsed = parse_pair_list(expr, context, |part, name| {
        let slot_idx = 2 * pair_idx + 1;
        pair_idx += 1;
        match part {
            ExpressionPart::Type(t) => match elaborate_type_expr(elaborator, t) {
                ElabResult::Done(kt) => Ok(kt),
                ElabResult::Park(producers) => {
                    parks.extend(producers);
                    // Placeholder; discarded under the Pending outcome. Lets the walk
                    // continue so every parking producer is collected in one pass.
                    Ok(KType::Any)
                }
                ElabResult::Unbound(msg) => Err(format!("{msg} in {context} for `{}`", name)),
            },
            // Legacy positional sigils (`:(List Tree)`) elaborate inline through the
            // threaded elaborator to keep the body's SCC context — `STRUCT Tree =
            // (children :(List Tree))` must lower Tree to `RecursiveRef("Tree")`. A
            // sub-Dispatch would lose that context and park on Tree's own slot.
            // Keyworded shapes (`:(LIST OF _)`, `:(FN ...)`) can't name the recursing
            // type and route through the standalone dispatcher safely.
            ExpressionPart::SigiledTypeExpr(boxed) => {
                if let Some(te) = try_synth_legacy(boxed) {
                    match elaborate_type_expr(elaborator, &te) {
                        ElabResult::Done(kt) => Ok(kt),
                        ElabResult::Park(producers) => {
                            parks.extend(producers);
                            Ok(KType::Any)
                        }
                        ElabResult::Unbound(msg) => {
                            Err(format!("{msg} in {context} for `{}`", name))
                        }
                    }
                } else {
                    let wrapped = KExpression::new(vec![Spanned::bare(
                        ExpressionPart::SigiledTypeExpr(boxed.clone()),
                    )]);
                    sub_dispatches.push((slot_idx, wrapped));
                    Ok(KType::Any)
                }
            }
            ExpressionPart::Future(crate::machine::model::KObject::KTypeValue(kt)) => Ok(kt.clone()),
            ExpressionPart::Future(other) => Err(format!(
                "{context} type for `{}` resolved to non-type value `{}`",
                name,
                other.summarize(),
            )),
            other => Err(format!(
                "{context} type for `{}` must be a type name token, got {}",
                name,
                other.summarize()
            )),
        }
    });
    match parsed {
        Err(msg) => FieldListOutcome::Err(msg),
        Ok(fields) => {
            if !parks.is_empty() || !sub_dispatches.is_empty() {
                FieldListOutcome::Pending { park_producers: parks, sub_dispatches }
            } else {
                FieldListOutcome::Done(fields)
            }
        }
    }
}

/// Synthesize the `TypeExpr` for a positional `:(<Head> <Arg>...)` sigil so the
/// field walker can elaborate self-recursive forms inline against the body's
/// threaded elaborator, preserving the SCC `current_decl` context that lowers
/// recursive names to `RecursiveRef`. Returns `None` for non-positional shapes;
/// the caller falls back to a sub-Dispatch for those.
fn try_synth_legacy(inner: &KExpression<'_>) -> Option<TypeExpr> {
    let parts = &inner.parts;
    let head = match &parts.first()?.value {
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => t,
        _ => return None,
    };
    let mut args: Vec<TypeExpr> = Vec::new();
    for p in &parts[1..] {
        match &p.value {
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => args.push(t.clone()),
            ExpressionPart::SigiledTypeExpr(boxed) => {
                args.push(try_synth_legacy(boxed)?);
            }
            _ => return None,
        }
    }
    let params = if args.is_empty() { TypeParams::None } else { TypeParams::List(args) };
    Some(TypeExpr {
        name: head.name.clone(),
        params,
        builtin_cache: std::cell::OnceCell::new(),
    })
}

//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::{NodeId, Scope};
use crate::parse::parse_pair_list;
use std::collections::HashSet;

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
            // Sigils (`:(LIST OF Tree)`, `:(MAP Tree -> _)`) sub-Dispatch through the
            // standalone dispatcher, which carries no SCC context, so self-references
            // are pre-resolved to `RecursiveRef` carriers first — `STRUCT Tree =
            // (children :(LIST OF Tree))` must lower Tree to `RecursiveRef("Tree")`.
            ExpressionPart::SigiledTypeExpr(boxed) => {
                let rewritten =
                    rewrite_threaded_self_refs(boxed, &elaborator.threaded, elaborator.scope);
                let wrapped = KExpression::new(vec![Spanned::bare(
                    ExpressionPart::SigiledTypeExpr(Box::new(rewritten)),
                )]);
                sub_dispatches.push((slot_idx, wrapped));
                Ok(KType::Any)
            }
            ExpressionPart::Future(crate::machine::model::KObject::KTypeValue(kt)) => {
                Ok(kt.clone())
            }
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
                FieldListOutcome::Pending {
                    park_producers: parks,
                    sub_dispatches,
                }
            } else {
                FieldListOutcome::Done(fields)
            }
        }
    }
}

/// Pre-resolve self-references inside a keyworded sigil body before it sub-Dispatches
/// into the standalone dispatcher, which carries no SCC threading context. Every bare
/// `Type(name)` leaf whose `name` is in `threaded` becomes a `Future(KTypeValue(
/// RecursiveRef(name)))` carrier — the same type-side transport `:(LIST OF Number)`
/// rides — so `STRUCT Tree = (children :(LIST OF Tree))` lowers `Tree` to
/// `RecursiveRef("Tree")` instead of parking on its own placeholder and closing a
/// scheduler-deadlock cycle. Recurses into nested sigils (`:(LIST OF (LIST OF Tree))`,
/// `:(MAP Tree -> Number)`); non-threaded names are left for the dispatcher to resolve.
fn rewrite_threaded_self_refs<'a>(
    inner: &KExpression<'a>,
    threaded: &HashSet<String>,
    scope: &Scope<'a>,
) -> KExpression<'a> {
    let parts = inner
        .parts
        .iter()
        .map(|p| {
            let value = match &p.value {
                ExpressionPart::Type(t) if threaded.contains(&t.name) => {
                    let obj = scope
                        .arena
                        .alloc(KObject::KTypeValue(KType::RecursiveRef(t.name.clone())));
                    ExpressionPart::Future(obj)
                }
                ExpressionPart::SigiledTypeExpr(b) => ExpressionPart::SigiledTypeExpr(Box::new(
                    rewrite_threaded_self_refs(b, threaded, scope),
                )),
                other => other.clone(),
            };
            Spanned {
                value,
                span: p.span,
            }
        })
        .collect();
    KExpression::new(parts)
}

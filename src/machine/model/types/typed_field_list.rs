//! Shared parser for `(<name> :<Type> <name> :<Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction). Under the Design-B sigil regime, the `:` is consumed by the
//! type sigil; each typed field lands as an `[Identifier, Type]` pair.

use super::ktype::KType;
use super::resolver::{elaborate_type_expr, ElabResult, Elaborator};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::model::Parseable;
use crate::parse::parse_pair_list;
use crate::machine::NodeId;

/// Result of one walk over a schema's field list.
pub enum FieldListOutcome<'a> {
    /// Every field type elaborated against the captured scope.
    Done(Vec<(String, KType<'a>)>),
    /// One or more field-type slots couldn't elaborate synchronously. `park_producers`
    /// names sibling placeholders the resolver wants to wait on; `sub_dispatches`
    /// names keyworded SigiledTypeExpr slots (`:(LIST OF Number)`, `:(FN ... -> ...)`,
    /// `:(FUNCTOR ... -> ...)`) that need to evaluate through the dispatcher to produce
    /// a type-side carrier. Each `(slot_idx_in_schema_parts, wrapped_expression)`
    /// pair tells the caller which schema slot to splice the resolved
    /// `KObject::KTypeValue` into before re-running the walk.
    ///
    /// The caller schedules a Combine over the merged producer list plus the
    /// scheduled sub-Dispatches and re-runs `parse_typed_field_list_via_elaborator`
    /// in the finish closure against a schema whose slots have been spliced with
    /// `Future(_)` carriers.
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
    /// Structural / unbound / cycle error.
    Err(String),
}

/// Phase-3 entry point used by STRUCT / UNION. Routes each field type through the
/// scheduler-aware [`elaborate_type_expr`], accumulating any parking producers and
/// pending sub-Dispatches across the whole field-list walk so the caller can install
/// one Combine for the merged set.
///
/// Three slot shapes admit:
/// - `ExpressionPart::Type(t)` — bare leaf or parameterized type token; runs through
///   the elaborator and either resolves to a `KType`, parks on a placeholder (added
///   to `park_producers`), or fails as unbound.
/// - `ExpressionPart::SigiledTypeExpr(_)` — the `:(...)` sigil. Scheduled as a
///   sub-Dispatch wrapped in its `SigiledTypeExpr` carrier so the dispatcher's
///   classifier handles every inner shape uniformly (TypeCall for legacy
///   `:(List Number)`, Keyworded for `:(LIST OF Number)` / `:(MAP K -> V)` /
///   `:(FN ...)` / `:(FUNCTOR ...)`, BareTypeLeaf for single-name `:Foo`,
///   FunctionValueCall for user-functor application like `:(MyF IntOrd)`).
/// - `ExpressionPart::Future(KObject::KTypeValue(kt))` — a sub-dispatched carrier
///   spliced back in by the caller's Combine finish; pre-resolved into `KType`.
///
/// Anything else surfaces as a structured `ShapeError`.
pub fn parse_typed_field_list_via_elaborator<'a>(
    expr: &KExpression<'a>,
    context: &str,
    elaborator: &mut Elaborator<'_, 'a>,
) -> FieldListOutcome<'a> {
    let mut parks: Vec<NodeId> = Vec::new();
    let mut sub_dispatches: Vec<(usize, KExpression<'a>)> = Vec::new();
    // `parse_pair_list` walks `[name_part, slot_part, name_part, slot_part, ...]`. The
    // slot's index in `expr.parts` is `2*pair_idx + 1`. Track the running pair index so
    // the closure can compute it.
    let mut pair_idx: usize = 0;
    let parsed = parse_pair_list(expr, context, |part, name| {
        let slot_idx = 2 * pair_idx + 1;
        pair_idx += 1;
        match part {
            ExpressionPart::Type(t) => match elaborate_type_expr(elaborator, t) {
                ElabResult::Done(kt) => Ok(kt),
                ElabResult::Park(producers) => {
                    parks.extend(producers);
                    // Placeholder KType — discarded when the caller routes through the
                    // Pending outcome. Keeps the pair-list walk going so we accumulate
                    // every parking producer in one pass.
                    Ok(KType::Any)
                }
                ElabResult::Unbound(msg) => Err(format!("{msg} in {context} for `{}`", name)),
            },
            // SigiledTypeExpr — two paths:
            //
            //  - **Legacy positional** (leaf head + leaf args, e.g. `:(List
            //    Tree)`, `:(Dict Str Number)`): synthesize the equivalent
            //    `TypeExpr` and elaborate INLINE through the field-walker's
            //    threaded elaborator. This preserves the STRUCT / UNION
            //    body's SCC context (`with_current_decl`, `with_threaded`),
            //    so a self-recursive shape like `STRUCT Tree = (children
            //    :(List Tree))` lowers Tree to `RecursiveRef("Tree")`. A
            //    sub-Dispatch through the standalone dispatcher would lose
            //    that context and park on Tree's own slot (cycle).
            //
            //  - **Keyworded / non-positional** (e.g. `:(LIST OF Number)`,
            //    `:(MAP Str -> Number)`, `:(FN ... -> ...)`): schedule a
            //    sub-Dispatch through the dispatcher. These shapes can't
            //    reference the recursing STRUCT/UNION by name through a
            //    registered overload's signature, so the standalone-dispatch
            //    path is safe. The resulting `KTypeValue` carrier splices
            //    back as `Future(_)` for the caller's re-walk.
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
            // Already-spliced sub-Dispatch result. The carrier's `ktype()` is the
            // resolved field type. `KTypeValue` is the canonical type-side carrier;
            // module / signature / user-type carriers project via `ktype()`.
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

/// Synthesize the equivalent `TypeExpr` for a positional `:(<Head> <Arg>...)`
/// sigil — the same legacy shape the dispatcher's `TypeCall` arm serves. Used
/// by the field walker to elaborate self-recursive forms (`STRUCT Tree =
/// (children :(List Tree))`) inline against the body's threaded elaborator,
/// preserving the SCC `current_decl` context that lowers Tree to
/// `RecursiveRef`. Returns `None` for non-positional shapes (keyworded heads,
/// sub-call args, nested non-leaf parts); the caller falls back to a
/// sub-Dispatch through the standalone dispatcher for those.
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



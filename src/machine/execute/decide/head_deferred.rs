//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first as a sub-dispatch and park the slot on it; once it
//! resolves, the finish classifies the value and applies it to `parts[1..]` via the shared
//! apply-a-callable tail. The `type_only` flag selects the admitted arm set (see
//! [`classify_head`]):
//!
//! - `HeadDeferred` (`type_only = false`): any `KFunction` value, or any type identity applied
//!   as a constructor.
//! - `TypeHeadDeferred` (head is a `:(...)` sigil, `type_only = true`): type identities only; a
//!   value-channel callable surfaces a type-shaped `TypeMismatch`.

use crate::machine::core::DepPlacement;
use crate::machine::core::location_from_expr;
use crate::machine::model::Carried;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{KError, KErrorKind};
use crate::source::Spanned;

use super::super::outcome::DepTerminal;
use super::apply_callable::{ResolvedCallable, apply_callable};
use super::ctx::DecideCtx;
use super::{Await, DepRequest, Outcome};
use crate::machine::AdoptSeam;
use crate::machine::model::RunRegistries;
use crate::scheduler::Deps;

pub(in crate::machine::execute) fn initial_expr<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let head = match expr.parts[0].value {
        WorkingPart::Ast(ExpressionPart::Expression(inner)) => {
            WorkingExpression::from_ast(ctx.current_scope().brand(), *inner)
        }
        // A synthesized head node is already working form.
        WorkingPart::Expression(inner) => *inner,
        _ => unreachable!("HeadDeferred shape implies nested Expression head"),
    };
    park_on_head(ctx, expr, head, false)
}

/// Wraps the sigil head as a one-part node rather than unwrapping it, so the type marker
/// survives the sub-dispatch.
pub(in crate::machine::execute) fn initial_type<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let head = match expr.parts[0].value {
        head @ WorkingPart::Ast(ExpressionPart::SigiledTypeExpr(_)) => {
            WorkingExpression::synthesized(
                ctx.current_scope().brand(),
                &[Spanned::bare(head)],
                &expr,
            )
        }
        _ => unreachable!("TypeHeadDeferred shape implies SigiledTypeExpr head"),
    };
    park_on_head(ctx, expr, head, true)
}

/// The general head lane: a head is callable whenever it *evaluates* to something callable, so any
/// part shape the `FunctionValueCall` name fast lane cannot resolve by a scope walk — a resolved
/// cell, a synthesized node, a slot still awaiting its sibling — routes here instead. Wraps the
/// part rather than unwrapping it, keeping whatever marker it carries (as [`initial_type`] does).
pub(in crate::machine::execute) fn defer_head<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let head = WorkingExpression::synthesized(ctx.current_scope().brand(), &[expr.parts[0]], &expr);
    park_on_head(ctx, expr, head, false)
}

/// The apply-a-callable tail the finish hands off to may itself re-park, so the finish must be
/// re-park-capable. A dep error short-circuits frameless in the harness before the finish runs, so
/// the finish only ever sees a resolved head.
fn park_on_head<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    head: WorkingExpression<'step>,
    type_only: bool,
) -> Outcome<'step> {
    let finish = move |ctx: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| {
        let head_terminal = terminals[0];
        // The head dep rests in a region this step already covers; lift it to an owned envelope
        // so the adopt below can fold its reach into the classified callable, which outlives this
        // finish. The callable arm adopts *as a callable* so it arrives fused to the reach it was
        // minted under; the fallback arm below takes the whole-value adopt instead.
        let head_delivered = ctx.current_scope().lift_spliced(&head_terminal.cell);
        let callable = match ctx
            .current_scope()
            .adopt_delivered_function(&head_delivered)
            .filter(|_| !type_only)
        {
            Some(function) => ResolvedCallable::Function(function),
            None => {
                let head = ctx
                    .current_scope()
                    .adopt_carried(&head_delivered, AdoptSeam::Retaining);
                match classify_head(head, type_only, &expr, ctx.registries()) {
                    Ok(c) => c,
                    Err(e) => return Outcome::Done(Err(e)),
                }
            }
        };
        apply_callable(ctx, callable, &expr)
    };
    Await::on(Deps::from_requests_in(
        [DepRequest::Dispatch {
            expr: head,
            placement: DepPlacement::OwnScope,
        }],
        ctx.scratch(),
    ))
    .finish_terminal(ctx.current_scope().brand(), finish)
}

/// A type identity is admitted as a constructor without pre-gating: `apply_callable`'s constructor
/// arm is the single authority on what a type admits when applied — unions, named type application,
/// every `SetMember` schema — and surfaces a `TypeMismatch: "constructible Type"` for the rest.
///
/// The caller's callable adopt takes an admitted value callable before this runs, so an object
/// reaching here is a diagnostic: under `type_only` the value channel is pruned and any object
/// surfaces a type-shaped `TypeMismatch`, otherwise it is a non-callable `DispatchFailed`.
fn classify_head<'step>(
    head: Carried<'step>,
    type_only: bool,
    expr: &WorkingExpression<'step>,
    registries: &RunRegistries,
) -> Result<ResolvedCallable<'step>, KError> {
    match head {
        Carried::Object(obj) => match obj {
            other if type_only => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: other.summarize(registries),
            })),
            other => Err(KError::new(KErrorKind::DispatchFailed {
                expr: other.summarize(registries),
                reason: "head evaluates to a non-callable value".to_string(),
                location: location_from_expr(expr),
            })),
        },
        // A head is resolved before it is classified, so an unlowered name names no callable.
        Carried::UnresolvedType(ti) => Err(KError::new(KErrorKind::UnboundName(
            crate::machine::model::render_label(ti.symbol(), registries),
        ))),
        Carried::Type(kt) => Ok(ResolvedCallable::Constructor { identity: kt }),
    }
}

//! The shared action harness. [`run_action`] drives the scheduler
//! from an [`Action`], the one place that touches `SchedulerHandle`. Both `KFunction::invoke`
//! (lowering an `ExecOutcome → Action`) and every `Action`-authored builtin route through it. The
//! peer of `dispatch/exec.rs::invoke`. The `Action` *types* live in
//! [`crate::machine::core::kfunction::action`].

use std::rc::Rc;

use crate::machine::core::kfunction::action::{
    Action, Dep, DepPlacement, FinishCtx, FramePlacement,
};
use crate::machine::core::kfunction::{BodyResult, CatchFinish, CombineFinish, SchedulerHandle};
use crate::machine::core::CallArena;
use crate::machine::NodeId;

/// Interpret an [`Action`] into the scheduler's `BodyResult` currency — the only code that calls
/// `SchedulerHandle`. A `Cont` / `CatchCont` returned by a finish is recursed into through the same
/// function. Returns a `BodyResult`; the caller maps that to a `NodeStep`.
pub fn run_action<'a, 's>(
    h: &mut dyn SchedulerHandle<'a, 's>,
    action: Action<'a>,
) -> BodyResult<'a> {
    match action {
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => BodyResult::Value(c),
        Action::Done(Err(e)) => BodyResult::Err(e),

        Action::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        } => {
            let frame: Option<Rc<CallArena>> = match frame_placement {
                FramePlacement::ReuseReserve { outer } => Some(h.acquire_tail_frame(outer)),
                FramePlacement::FreshChild { frame } => Some(frame),
                FramePlacement::Inherit => None,
            };
            let n_leading = leading.len();
            // The body's non-tail statements dispatch as siblings via the shared
            // `SchedulerHandle::dispatch_body_statements` — the same primitive `KFunction::invoke`
            // uses. The caller (here) tail-replaces into the last statement separately.
            if !leading.is_empty() {
                let cart = frame
                    .clone()
                    .expect("Action::Tail with leading requires a frame");
                h.dispatch_body_statements(&cart, leading);
            }
            // A block-entering tail sits above the params (`1`) or the leading siblings (`N`); a
            // frameless continuation keeps the slot's block at index `0`.
            let body_index = if block_entry.is_some() {
                n_leading + 1
            } else {
                0
            };
            BodyResult::Tail {
                expr: tail,
                frame,
                function: contract,
                block_entry,
                body_index,
            }
        }

        Action::Combine { deps, finish } => {
            // `Dispatch` deps → owned sub-slots (an `InScope` body fans out one per statement via
            // `enter_body_block`); `Existing` deps → park-producers the combine reads but doesn't own.
            let mut owned = Vec::new();
            let mut park = Vec::new();
            for dep in deps {
                match dep {
                    Dep::Existing(id) => park.push(id),
                    Dep::Dispatch { expr, placement } => match placement {
                        DepPlacement::InScope(scope) => {
                            owned.extend(h.enter_body_block(scope, expr))
                        }
                        DepPlacement::OwnScope => owned.push(h.add_dispatch_here(expr)),
                        DepPlacement::ActiveFrame => owned.push(h.add_dispatch_in_frame(expr)),
                    },
                }
            }
            let wrapped: CombineFinish<'a> = Box::new(move |sched, results| {
                let fctx = FinishCtx {
                    scope: sched.current_scope(),
                };
                let next = finish(&fctx, results);
                run_action(sched, next)
            });
            BodyResult::DeferTo(h.add_combine_here(owned, park, wrapped))
        }

        Action::Catch { watched, finish } => {
            let from = dispatch_dep(h, watched);
            let wrapped: CatchFinish<'a> = Box::new(move |sched, result| {
                let fctx = FinishCtx {
                    scope: sched.current_scope(),
                };
                let next = finish(&fctx, result);
                run_action(sched, next)
            });
            BodyResult::DeferTo(h.add_catch_here(from, wrapped))
        }
    }
}

/// Realize a [`Dep`] to a producer `NodeId`: dispatch a `Dispatch` (per its placement) → an owned
/// sub-slot; an `Existing` is already a producer the builtin found in scope.
fn dispatch_dep<'a, 's>(h: &mut dyn SchedulerHandle<'a, 's>, dep: Dep<'a>) -> NodeId {
    match dep {
        Dep::Existing(id) => id,
        Dep::Dispatch { expr, placement } => match placement {
            DepPlacement::OwnScope => h.add_dispatch_here(expr),
            DepPlacement::ActiveFrame => h.add_dispatch_in_frame(expr),
            // A single watched expr enters a fresh lexical block over `scope` (TRY's
            // `child_under` body scope), so an inner `LET` stays local. One statement → one id.
            DepPlacement::InScope(scope) => h
                .enter_block(scope.id, vec![expr], scope)
                .into_iter()
                .next()
                .expect("enter_block of one statement yields one node"),
        },
    }
}

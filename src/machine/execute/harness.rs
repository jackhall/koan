//! The shared action harness. [`run_action`] drives the scheduler
//! from an [`Action`], the one place that touches `SchedulerHandle`. Both `KFunction::invoke`
//! (lowering an `ExecOutcome → Action`) and every `Action`-authored builtin route through it. The
//! peer of `dispatch/exec.rs::invoke`. The `Action` *types* live in
//! [`crate::machine::core::kfunction::action`].

use crate::machine::core::kfunction::action::{Action, Dep, DepPlacement, FinishCtx};
use crate::machine::NodeId;

use super::nodes::{NodeOutput, NodeWork};
use super::outcome::{forward_owned, Outcome};
use super::{CatchFinish, CombineFinish, SchedulerHandle};

/// Interpret an [`Action`] into the scheduler's [`Outcome`] currency — the only code that calls
/// `SchedulerHandle`. A `Cont` / `CatchCont` returned by a finish is recursed into through the same
/// function. Returns an `Outcome`; the harness applies it.
pub(in crate::machine::execute) fn run_action<'a, 's>(
    h: &mut dyn SchedulerHandle<'a, 's>,
    action: Action<'a>,
) -> Outcome<'a> {
    match action {
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => Outcome::Done(NodeOutput::Value(c)),
        Action::Done(Err(e)) => Outcome::Done(NodeOutput::Err(e)),

        Action::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        } => {
            // A block-entering tail sits above the params (`1`) or the leading siblings (`N`); a
            // frameless continuation keeps the slot's block at index `0`. The harness resolves
            // `frame_placement` to a cart and dispatches `leading` against it — this decide names
            // the work but issues no write.
            let body_index = if block_entry.is_some() {
                leading.len() + 1
            } else {
                0
            };
            Outcome::Continue {
                work: NodeWork::dispatch(tail),
                frame: frame_placement,
                contract,
                block_entry,
                leading,
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
            forward_owned(h.add_combine_here(owned, park, wrapped))
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
            forward_owned(h.add_catch_here(from, wrapped))
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

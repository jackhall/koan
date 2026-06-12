//! The shared action-harness (WIP, `action-harness` feature). [`run_action`] drives the scheduler
//! from an [`Action`], the one place that touches `SchedulerHandle`. Both `KFunction::invoke`
//! (lowering an `ExecOutcome → Action`) and every `Action`-authored builtin route through it. The
//! peer of `dispatch/exec.rs::invoke`. The `Action` *types* live in
//! [`crate::machine::core::kfunction::action`]. See `scratch/action-spec.md` for the survey + audit.
#![allow(dead_code)]

use std::rc::Rc;

use crate::machine::core::kfunction::action::{Action, Dep, DepPlacement, FinishCtx, FramePlacement};
use crate::machine::core::kfunction::{BodyResult, CatchFinish, SchedulerHandle};
use crate::machine::core::CallArena;
use crate::machine::NodeId;

/// Interpret an [`Action`] into the scheduler's `BodyResult` currency — the only code that calls
/// `SchedulerHandle`. A `Cont` / `CatchCont` returned by a finish is recursed into through the same
/// function. Returns a `BodyResult`; the caller maps that to a `NodeStep`.
pub fn run_action<'a, 's>(h: &mut dyn SchedulerHandle<'a, 's>, action: Action<'a>) -> BodyResult<'a> {
    match action {
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => BodyResult::Value(c),
        Action::Done(Err(e)) => BodyResult::Err(e),

        Action::Tail {
            leading,
            tail,
            contract,
            frame_placement,
        } => {
            // Spike: only the no-`leading`, no-block-entry shape (EVAL). Leading siblings + a fresh
            // lexical block need an `Action::Tail` `block_entry` field — added when MATCH / the
            // FN-body tails are ported.
            assert!(
                leading.is_empty(),
                "run_action: Action::Tail with leading siblings not yet implemented"
            );
            let frame: Option<Rc<CallArena>> = match frame_placement {
                FramePlacement::ReuseReserve { outer } => Some(h.acquire_tail_frame(outer)),
                FramePlacement::FreshChild { frame } => Some(frame),
                FramePlacement::Inherit => None,
            };
            BodyResult::Tail {
                expr: tail,
                frame,
                function: contract,
                block_entry: None,
                body_index: 0,
            }
        }

        Action::Combine { .. } => {
            todo!("run_action: Action::Combine (dispatch deps, add_combine_in_frame, finish→run_action)")
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
            DepPlacement::InScope(scope) => h.add_dispatch(expr, scope),
            DepPlacement::WithChain(_) => {
                todo!("dispatch_dep: DepPlacement::WithChain (TRY arms) not yet implemented")
            }
        },
    }
}

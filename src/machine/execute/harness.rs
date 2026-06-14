//! The shared action harness. [`run_action`] turns an [`Action`] into the scheduler's [`Outcome`]
//! currency — a pure decide that reads a [`SchedulerView`] and issues no graph write. Both
//! `KFunction::invoke` (lowering an `ExecOutcome → Action`) and every `Action`-authored builtin
//! route through it. The peer of `dispatch/exec.rs::invoke`. The `Action` *types* live in
//! [`crate::machine::core::kfunction::action`].

use crate::machine::core::kfunction::action::{Action, Dep, FinishCtx, FramePlacement};

use super::nodes::NodeOutput;
use super::outcome::{Continuation, DispatchDep, Outcome};
use super::{CatchFinish, CombineFinish};

/// Lower an [`Action`] into the scheduler's [`Outcome`] currency — a pure `Action -> Outcome`
/// transform that reads nothing: a `Combine`/`Catch` declares its deps (and a wrapped finish that
/// recurses `run_action` on the `Cont`/`CatchCont` it produces) as a [`Outcome::ParkThenContinue`],
/// and the harness submits and applies. Every scheduler read the body needs is deferred into the
/// finish, which sees a read-only [`SchedulerView`](super::dispatch::SchedulerView) at wake.
pub(in crate::machine::execute) fn run_action<'run>(action: Action<'run>) -> Outcome<'run> {
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
            // frameless continuation keeps the slot's block at index `0`.
            let body_index = if block_entry.is_some() {
                leading.len() + 1
            } else {
                0
            };
            if leading.is_empty() {
                // No leading statements: tail-replace directly into the arm body.
                return Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: frame_placement,
                    contract,
                    block_entry,
                    body_index,
                };
            }
            // Leading statements become owned siblings in the arm's frame (one `BodyBlock` dep);
            // the slot parks on them so they run — and cascade-free — before the tail continues,
            // keeping the side-effect order and the frame uniqueness TCO reuse needs. An arm that
            // carries leading statements always mints its own `FreshChild` frame (`branch_walk`),
            // so the placement resolves to a concrete cart here without a scheduler write.
            let frame = match frame_placement {
                FramePlacement::FreshChild { frame } => frame,
                _ => unreachable!(
                    "an action Tail with leading statements always carries a FreshChild frame"
                ),
            };
            let body_frame = frame.clone();
            let finish: CombineFinish<'run> = Box::new(move |_view, _results| Outcome::Continue {
                work: super::dispatch::decide(tail),
                frame: FramePlacement::FreshChild { frame: body_frame },
                contract,
                block_entry,
                body_index,
            });
            Outcome::ParkThenContinue {
                deps: vec![DispatchDep::BodyBlock {
                    frame,
                    statements: leading,
                }],
                park_count: 0,
                cont: Continuation::Combine(finish),
                dep_error_frame: None,
                free: Vec::new(),
            }
        }

        Action::Combine { deps, finish } => {
            // `Existing` deps are park-producers the combine reads but doesn't own; `Dispatch`
            // deps are owned sub-slots (an `InScope` body fans out one per statement at apply
            // time). The harness orders the realized deps `[park..., owned...]`; `park_count` is
            // the park prefix length. The wrapped finish recurses `run_action` on the `Cont`.
            let mut park: Vec<DispatchDep<'run>> = Vec::new();
            let mut owned: Vec<DispatchDep<'run>> = Vec::new();
            for dep in deps {
                match dep {
                    Dep::Existing(id) => park.push(DispatchDep::Existing(id)),
                    Dep::Dispatch { expr, placement } => {
                        owned.push(DispatchDep::Dispatch { expr, placement })
                    }
                }
            }
            let park_count = park.len();
            park.extend(owned);
            let wrapped: CombineFinish<'run> = Box::new(move |view, results| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                };
                run_action(finish(&fctx, results))
            });
            Outcome::ParkThenContinue {
                deps: park,
                park_count,
                cont: Continuation::Combine(wrapped),
                dep_error_frame: None,
                free: Vec::new(),
            }
        }

        Action::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a Combine body's fan-out.
            let wrapped: CatchFinish<'run> = Box::new(move |view, result| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                };
                run_action(finish(&fctx, result))
            });
            Outcome::ParkThenContinue {
                deps: Vec::new(),
                park_count: 0,
                cont: Continuation::Catch {
                    watched,
                    finish: wrapped,
                },
                dep_error_frame: None,
                free: Vec::new(),
            }
        }
    }
}

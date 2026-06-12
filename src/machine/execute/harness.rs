//! **DESIGN SKETCH — not wired into the live call path.** The shared action-harness:
//! [`run_action`] drives the scheduler from a [`Action`], the one place that touches
//! `SchedulerHandle`. Both `KFunction::invoke` (lowering an `ExecOutcome → Action`) and every
//! `Action`-authored builtin route through it. The peer of `dispatch/exec.rs::invoke`.
//!
//! The `Action` *types* live in [`crate::machine::core::kfunction::action`] (next to `ExecOutcome` /
//! `Body`, since they reference only core/model types). This module is the scheduler-aware half.
//!
//! Interpreter bodies are `todo!()` on purpose — this pins the boundary, not the implementation.
//! See `scratch/action-spec.md` for the survey + audit this was distilled from.
#![allow(dead_code)]

use crate::machine::core::kfunction::action::Action;
use crate::machine::core::kfunction::{BodyResult, SchedulerHandle};

/// The one shared harness: run a [`Action`] into the scheduler's `BodyResult` currency — the
/// only code that calls `SchedulerHandle`. `KFunction::invoke` calls it with a lowered
/// `ExecOutcome`; a builtin call site calls the `ActionFn` to get a `Action`, then calls this. A
/// `Cont` / `CatchCont` returned by a finish is recursed into through the same function.
pub fn run_action<'a, 's>(
    h: &mut dyn SchedulerHandle<'a, 's>,
    action: Action<'a>,
    idx: usize,
) -> BodyResult<'a> {
    let _ = (h, idx);
    match action {
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => BodyResult::Value(c),
        Action::Done(Err(e)) => BodyResult::Err(e),

        // Mint the cart per `frame_placement` (install a `FreshChild`, `acquire_tail_frame` a
        // `ReuseReserve`), dispatch `leading` per each Dep's placement, then
        // `tail_with_frame_contract(tail, cart, contract, body_index)`.
        Action::Tail { .. } => todo!("Tail: install/acquire cart, dispatch leading, tail-replace"),

        // `Dep::Dispatch` → owned_subs (per `DepPlacement`), `Dep::Existing` → park_producers,
        // `add_combine*` with a `CombineFinish` that builds a `FinishCtx` from the wake scope, calls
        // the `Cont`, and `run_action`s the returned `Action`. Slot `DeferTo`s.
        Action::Combine { .. } => todo!("Combine: dispatch deps, add_combine, finish wraps Cont→run_action"),

        // Dispatch `watched`, `add_catch*` with a `CatchFinish` that calls the `CatchCont` and
        // interprets the returned `Action`. Slot `DeferTo`s.
        Action::Catch { .. } => todo!("Catch: dispatch watched, add_catch, finish wraps CatchCont→run_action"),
    }
}

// The `ExecOutcome → Action` lowering (`KFunction::invoke`'s half) is intentionally omitted: it
// bridges `ExecOutcome`'s two lifetimes (`'ast`/`'frame`) to `Action`'s one and is the spike's first
// compile-check. `DeferredExprTail` lowers to
// `Action::Combine{ deps:[Dispatch{type_expr, OwnScope}], finish: |_, r| Action::Tail{..} }`.

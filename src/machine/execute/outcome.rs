//! The unified scheduler-step currency.
//!
//! Every node step — a fresh dispatch decide, a finish, a builtin body, an invoke — decides
//! against a read-only [`SchedulerView`](super::dispatch::SchedulerView) and **returns** an
//! [`Outcome`]; [`KoanRuntime::apply_outcome`](super::runtime::KoanRuntime) is the sole place that
//! turns an outcome into the scheduler-graph writes it implies and the terminal
//! [`NodeStep`](super::nodes::NodeStep). The scheduler never learns *what* a step ran (dispatch /
//! invoke / builtin) nor *whether* it ran before — only a read view in and an outcome out.
//!
//! The taxonomy is AST-free — no variant names a `KFunction` or a `KExpression`:
//! - [`Outcome::Done`] — the node dies, producing a value to lift or an error.
//! - [`Outcome::Continue`] — the node lives; replace its work and run again immediately (no park).
//!   A resolved call folds into this: the producer installs the per-call cart (its frame placement)
//!   and the work re-decides via the folded `invoke` / re-resolve closure on the next pop — so the
//!   dispatch→execution hand-off is a dep-free `Continue`, not a distinct trigger.
//! - [`Outcome::ParkThenContinue`] — park on deps; on resolve run a [`Continuation`] that yields
//!   another outcome.
//! - [`Outcome::Forward`] — splice the slot out as an alias of an existing producer.

use crate::machine::core::kfunction::action::{Dep, FramePlacement};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::ScopeId;
use crate::scheduler::{reattach_slice, reattach_value, Erased, Reattachable};
use crate::machine::model::values::{Carried, CarriedFamily, KObject, ResultCarriedFamily};
use crate::machine::{KError, NodeId, TraceFrame};

use super::dispatch::{propagate_dep_error, DepRequest, ResumeFn, SchedulerView};
use super::nodes::NodeWork;
use super::runtime::KoanWorkload;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload), mirroring `NodeStep::Replace`; boxing the hot
// continuation path to balance variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'s> {
    /// The node dies with a value or an error. The value is bound to the per-step cart lifetime
    /// `'s` — the decide-surface lifetime — born in the node's own per-call frame (a builtin
    /// allocates it there, a forwarded dep arrives already lifted into it) and relocated across each
    /// dep edge by the consumer-pull lift into the consuming node's frame.
    Done(Result<Carried<'s>, KError>),
    /// The node lives: install `work` and run again immediately (no park). `frame` rotates the
    /// per-call cart (`Inherit` keeps it; `ReuseReserve`/`FreshChild` install a new one — the
    /// harness resolves the placement to a cart); `contract` / `block_entry` / `body_index` carry
    /// the tail-call chain payload, all keep-first. A body's non-tail (leading) statements are NOT
    /// carried here: a producer with leading statements parks on them as owned deps (a
    /// [`DepRequest::BodyBlock`]) and emits this `Continue` only from the resolving finish, so the
    /// leading siblings cascade-free before the tail-replace — restoring frame uniqueness for TCO
    /// reuse. `body_index` already accounts for their count.
    Continue {
        work: NodeWork<KoanWorkload>,
        frame: FramePlacement<'s>,
        contract: Option<ReturnContract<'s>>,
        block_entry: Option<ScopeId>,
        body_index: usize,
    },
    /// Park the slot on `deps` and run `cont` when they resolve. `deps` layout is
    /// `[park_producers..., owned_subs...]`; `park_count` is the park-producer prefix length
    /// (`Notify` edges, kept alive), the suffix installs as `Owned` (cascade-freed). For a
    /// [`Continuation::Resume`] every dep parks (notify-only).
    /// `dep_error_frame` is attached to a dep-error short-circuit (dep-finish-style) before the
    /// finish runs.
    ParkThenContinue {
        deps: Vec<DepRequest<'s>>,
        park_count: usize,
        cont: Continuation<'s>,
        dep_error_frame: Option<TraceFrame>,
    },
    /// The slot's result *is* `producer`'s result (a bare name resolving to a binding). Rather than
    /// installing a forwarding node, the harness splices the slot out: if `producer` is ready the
    /// slot finalizes with its terminal directly; otherwise the slot's consumers are moved onto
    /// `producer`'s notify list and the slot becomes an alias that reads through to `producer`. So
    /// the single-producer invariant holds with `producer` as the sole producer — no duplicate
    /// forwarding slot.
    Forward(NodeId),
}

/// What a [`Outcome::ParkThenContinue`] runs once its deps resolve. The shapes are the closed set
/// of "what happens on wake":
/// - `Finish` installs a [`NodeWork`](super::nodes::NodeWork) that short-circuits on the first
///   errored dep (under the [`Outcome::ParkThenContinue::dep_error_frame`]) and otherwise hands the
///   resolved dep values to a [`DepFinish`]. This is both a dispatch decide's re-park/splice (it
///   carries the consuming call's frame, or `None` frameless) and the action-harness / literal
///   dep-finishes ([`run_action`](super::runtime::run_action)'s `Action::AwaitDeps`, the literal
///   builders — labelled [`dep_error_frame`]). Its finish consumes the dep values, runs against a
///   read-only [`SchedulerView`], and returns another [`Outcome`] (it may itself re-park).
/// - `Catch` is the action-harness catch ([`run_action`](super::runtime::run_action)'s
///   `Action::Catch`): the slot becomes a [`NodeWork`](super::nodes::NodeWork) watching the realized `watched` dep;
///   the harness owns that producer. `watched`'s placement is realized at apply time (an `InScope`
///   watched enters a fresh single-statement block, unlike a dep-finish body's fan-out).
/// - `Resume` re-runs the parked dispatch decide (the `ParkSelf` shape) through the opaque
///   [`ResumeFn`] closure the parking decide captured; `carrier` is the parked expression's
///   pre-rendered summary the drain-end deadlock report surfaces (`None` when the park carries no
///   renderable form). On apply the slot becomes a resume
///   [`NodeWork`](super::nodes::NodeWork).
///
/// (A bare-name forward is not a continuation — it splices the slot out via
/// [`Outcome::Forward`], never parking on a dep.)
pub(in crate::machine::execute) enum Continuation<'run> {
    Finish(DepFinish<'run>),
    Catch {
        watched: Dep<'run>,
        finish: CatchFinish<'run>,
    },
    Resume {
        carrier: Option<String>,
        resume: ResumeFn<'run>,
    },
}

/// Host-side value-only closure run by a dep-finish [`NodeWork`](super::nodes::NodeWork) once its
/// deps resolve without error. Receives the dep terminals in submission
/// order as [`Carried`] (an object or a type flowing in the type channel); static elements are
/// captured in the closure. A value-consuming finish calls `.object()` on each; a type-resolving
/// dep (a VAL type, an FN return type, a field type) arrives as [`Carried::Type`]. The finish
/// decides against a read-only [`SchedulerView`] and returns an [`Outcome`] the harness applies —
/// it issues no graph write of its own.
pub(in crate::machine::execute) type DepFinish<'a> =
    Box<dyn for<'v> FnOnce(&SchedulerView<'a, 'v>, &[Carried<'a>]) -> Outcome<'a> + 'a>;

/// The error-frame label a dep-finish attaches when a dependency errors — an action-harness combine
/// (a fanned-out FN arg / arm body) or a literal builder (a list element / dict value). A dispatch
/// finish carries the consuming call's own frame instead, so this is the fallback label for the
/// frameless dep-finish paths.
pub(in crate::machine::execute) fn dep_error_frame() -> TraceFrame {
    TraceFrame::bare("<deps>", "deps")
}

/// Host-side closure for a catch [`NodeWork`](super::nodes::NodeWork). Receives the watched slot's terminal as a
/// `Result` so the closure can branch on either outcome, plus a read-only [`SchedulerView`].
pub(in crate::machine::execute) type CatchFinish<'a> = Box<
    dyn for<'v> FnOnce(&SchedulerView<'a, 'v>, Result<&'a KObject<'a>, KError>) -> Outcome<'a>
        + 'a,
>;

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries. It receives the dep terminals in submission order
/// as `Result`s (an errored dep is *not* short-circuited by the handler — the continuation decides),
/// a read-only [`SchedulerView`], and the slot's own index, and returns an [`Outcome`] the harness
/// applies. The per-family behaviors (combine short-circuit, catch recover, dispatch decide) are
/// built into the closure by the combinators below, so the node itself never branches.
pub(in crate::machine::execute) type NodeCont<'a> = Box<
    dyn for<'v> FnOnce(&SchedulerView<'a, 'v>, &[Result<Carried<'a>, KError>], usize) -> Outcome<'a>
        + 'a,
>;

/// `Reattachable` family for the [`NodeCont`] continuation. Layout-invariant: `NodeCont<'r>` is a
/// `Box<dyn …>` fat pointer whose representation never depends on `'r`.
pub(in crate::machine::execute) struct ContFamily;

// SAFETY: `NodeCont<'r>` is one type generic only in `'r` (a boxed trait object); its fat-pointer
// layout is identical for every `'r`.
unsafe impl Reattachable for ContFamily {
    type At<'r> = NodeCont<'r>;
}

/// A [`NodeCont`] with its captured `'run` lifetime erased to `'static` for storage on a
/// lifetime-free node, re-anchored against the node's cart `Rc` before the single-shot run. The
/// continuation captures run-lived data (the parked AST, a finish closure's captured scope) living
/// in the run arena or a strict ancestor of the slot's per-call cart, which the node's
/// [`CallFrame`](super::nodes::CallFrame) cart `Rc` keeps live across the step — the liveness
/// witness the caller holds across `reattach`. Unlike the `Copy` value/contract carriers the
/// continuation is a `Box<dyn FnOnce>` consumed once, so the family is not `Copy` and `reattach`
/// takes `self` by value. See [`Erased`].
pub(in crate::machine::execute) type ErasedCont = Erased<ContFamily>;

/// Dep-finish continuation: short-circuit on the first errored dep (labelled with `dep_error_frame`),
/// else hand the resolved [`Carried`] values to a value-only [`DepFinish`]. The short-circuit
/// the handler used to do is now this combinator's job, so the node stays uniform.
pub(in crate::machine::execute) fn short_circuit<'a>(
    dep_error_frame: Option<TraceFrame>,
    finish: DepFinish<'a>,
) -> NodeCont<'a> {
    Box::new(move |view, results, _idx| {
        let mut values: Vec<Carried<'_>> = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(c) => values.push(*c),
                Err(e) => {
                    return Outcome::Done(Err(propagate_dep_error(e, dep_error_frame.clone())))
                }
            }
        }
        // The deps were pull-lifted into this node's frame at the same cart-scale lifetime the
        // finish runs at — no re-exposure across the boundary, the splice slot and value share `'a`.
        finish(view, &values)
    })
}

/// Catch continuation: hand the single watched dep's terminal (Value or Err) to a [`CatchFinish`]
/// without short-circuiting, so the closure can recover or re-raise.
pub(in crate::machine::execute) fn catch_cont<'a>(finish: CatchFinish<'a>) -> NodeCont<'a> {
    Box::new(move |view, results, _idx| {
        let result = match &results[0] {
            // The watched terminal shares the cart-scale `'a` lifetime the finish runs at.
            Ok(c) => Ok(c.object()),
            // Frameless: the recovery-site dispatch attaches its own frame.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        finish(view, result)
    })
}

/// Dispatch-decide continuation: a [`ResumeFn`] takes no dep values (it reads the view and spawns /
/// re-resolves), so its deps are park-only and the results slice is ignored.
pub(in crate::machine::execute) fn ignore_results<'a>(resume: ResumeFn<'a>) -> NodeCont<'a> {
    Box::new(move |view, _results, idx| resume(view, idx))
}

/// Reattach a step-bound `'s` terminal up to `'run` for storage in the slot table. The value is
/// born in the producer's per-call frame (or is genuinely `'run`-lived), and the harness pins that
/// frame's `Rc` alongside the terminal in the scheduler's finalized-slot state until the slot is
/// freed, so the stored `'run` view cannot outlive its backing arena. The reattach is needed only
/// because `Carried` is invariant; this is the held-`Rc` re-exposure seam.
pub(in crate::machine::execute) fn pin_carried_to_run<'run>(value: Carried<'_>) -> Carried<'run> {
    // SAFETY: lifetime-only reattach; the frame `Rc` co-stored in `SlotState::Done` heap-pins the
    // backing arena for as long as the terminal is readable. See the doc comment.
    unsafe { reattach_value::<CarriedFamily>(value) }
}

/// Reattach the consumer's pull-lifted dep terminals to the cart-scale lifetime `'s` the
/// continuation runs at. Each value was just copied into this consumer's per-call frame and dies
/// with it; the reattach is needed only because `Carried` is invariant, so a lifetime-only re-anchor
/// to the lifetime the cart `Rc` witnesses is sound.
pub(in crate::machine::execute) fn deps_at_step<'b, 'run, 's>(
    results: &'b [Result<Carried<'run>, KError>],
) -> &'b [Result<Carried<'s>, KError>] {
    // SAFETY: lifetime-only reattach of an invariant carrier to the cart-witnessed lifetime the
    // values genuinely live at (they die with the consumer frame). See the doc comment.
    unsafe { reattach_slice::<ResultCarriedFamily>(results) }
}

#[cfg(test)]
mod erased_cont_tests {
    //! Miri coverage for the [`ErasedCont`] continuation erasure: the test pins the
    //! erase → reattach → invoke round-trip under tree borrows; logical assertions are minimal —
    //! it fails when Miri reports UB, not on values.

    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::core::{CallArena, RuntimeArena};
    use crate::scheduler::Scheduler;

    /// A continuation capturing cart-ancestor data (a `&KObject` in the run arena — a strict
    /// ancestor of the cart) is erased to `'static`, reattached against the cart `Rc` for one step,
    /// and then *invoked*, so tree borrows checks the capture read through the lifetime-fabricated
    /// box. The cart's `outer` chain pins the ancestor arena, so the step-scale fabrication is
    /// honest. Mirrors the erase → reattach transmute pair plus the single-shot call site
    /// (`run_step`); fails on UB, not values.
    #[test]
    fn erased_cont_reattach_roundtrip() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        // The captured value lives in the run arena — the ancestor the cart's `outer` chain pins.
        let captured: &KObject = arena.alloc_object(KObject::Number(7.0));
        // Held live to the end of the test so it witnesses the reattach below.
        let _cart = CallArena::new(scope, None);

        let cont: NodeCont = Box::new(move |_view, _results, _idx| {
            // Read the run-lived capture through the reattached box.
            assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
            Outcome::Done(Err(KError::new(crate::machine::KErrorKind::ShapeError(
                "ran".to_string(),
            ))))
        });
        let erased = ErasedCont::erase(cont);
        // Reattach against the cart `Rc` the test holds live, then run the single-shot continuation.
        let reattached: NodeCont<'_> = unsafe { erased.reattach() };
        let sched = Scheduler::new();
        let ambient = crate::machine::execute::ambient::AmbientContext::default();
        let view = SchedulerView::new(&sched, &ambient);
        let out = reattached(&view, &[], 0);
        assert!(matches!(out, Outcome::Done(Err(_))));
        // Mutate the arena through a sibling pointer after the call to catch a stacked-borrow regression.
        let _other = arena.alloc_object(KObject::Number(8.0));
        assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
    }
}

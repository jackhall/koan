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
use crate::machine::model::values::{Carried, CarriedFamily, KObject};

use crate::machine::{FrameSet, KError, NodeId, TraceFrame};
use crate::witnessed::reattachable;
use crate::witnessed::{Sealed, Witnessed};

use super::dispatch::{propagate_dep_error, DepRequest, ResumeFn, SchedulerView};
use super::lift::{reached_frame, relocate_carried};
use super::nodes::NodeWork;
use super::runtime::KoanWorkload;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload), mirroring `NodeStep::Replace`; boxing the hot
// continuation path to balance variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'step> {
    /// The node dies with a value or an error. The value is bound to the per-step cart lifetime
    /// `'step` — the decide-surface lifetime — born in the node's own per-call frame (a builtin
    /// allocates it there, a forwarded dep arrives already lifted into it) and relocated across each
    /// dep edge by the consumer-pull lift into the consuming node's frame.
    Done(Result<Carried<'step>, KError>),
    /// The node dies with a value **built inside the witness closure** — a
    /// [`Witnessed`](crate::witnessed::Witnessed) carrier already naming every region it reaches
    /// (`yoke` / `merge` / the aggregate fold at the alloc site), so `finalize` seals it without an
    /// asserted-co-location [`Witnessed::new`](crate::witnessed::Witnessed::new). The object-family
    /// terminal; the type channel and every error stay on [`Done`](Self::Done) until the type family
    /// inverts. The carrier is lifetime-free, so this arm carries no `'step`.
    DoneWitnessed(Witnessed<CarriedFamily, FrameSet>),
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
        frame: FramePlacement<'step>,
        contract: Option<ReturnContract<'step>>,
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
        deps: Vec<DepRequest<'step>>,
        park_count: usize,
        continuation: Continuation<'step>,
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
pub(in crate::machine::execute) enum Continuation<'step> {
    Finish(DepFinish<'step>),
    Catch {
        watched: Dep<'step>,
        finish: CatchFinish<'step>,
    },
    Resume {
        carrier: Option<String>,
        resume: ResumeFn<'step>,
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
    Box<dyn for<'view> FnOnce(&SchedulerView<'a, 'view>, &[Carried<'a>]) -> Outcome<'a> + 'a>;

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
    dyn for<'view> FnOnce(&SchedulerView<'a, 'view>, Result<&'a KObject<'a>, KError>) -> Outcome<'a>
        + 'a,
>;

/// A resolved dep terminal as the continuation receives it, **un-relocated**. It holds the producer
/// slot's own [`Sealed`] carrier (a [`duplicate`](crate::witnessed::Sealed::duplicate) — the producer
/// keeps its terminal for other consumers), so a **construction finish** folds the dep *witnessed* via
/// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into), its reach named on the carrier
/// by construction (the [`alloc` construction
/// inversion](../../../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)). `value` is the same
/// value re-anchored **live at the step brand** (read out of the producer slot, pinned by the step
/// open) for the **value-copy** finishes still on the bare channel (the type channel), which relocate
/// it into the consumer region via [`relocate_dep_into_consumer`] (retaining a surviving closure /
/// module borrow through [`reached_frame`](super::lift::reached_frame)). The dep's reach is read off
/// the carrier (`carrier.witness()`) and unioned into the consumer-step `pin` before the open.
pub(in crate::machine::execute) struct DepTerminal<'a> {
    pub(in crate::machine::execute) value: Carried<'a>,
    pub(in crate::machine::execute) carrier: Sealed<CarriedFamily, FrameSet>,
}

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries. It receives the dep terminals in submission order
/// as `Result`s (an errored dep is *not* short-circuited by the handler — the continuation decides),
/// a read-only [`SchedulerView`], and the slot's own index, and returns an [`Outcome`] the harness
/// applies. The per-family behaviors (combine short-circuit, catch recover, dispatch decide) are
/// built into the closure by the combinators below, so the node itself never branches.
pub(in crate::machine::execute) type NodeContinuation<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            &[Result<DepTerminal<'a>, KError>],
            usize,
        ) -> Outcome<'a>
        + 'a,
>;

/// `Reattachable` family for the [`NodeContinuation`] continuation — the scheduler stores it erased
/// (`Erased<ContinuationFamily>`) on a lifetime-free node and the workload opens it once per step via
/// the consuming externally-witnessed [`SealedExtern::open`](crate::witnessed::SealedExtern::open)
/// before the single-shot run. The continuation captures run-lived data (the parked AST, a finish
/// closure's captured scope) living in the run region or a strict ancestor of the slot's per-call
/// cart, which the node's [`NodeFrame`](super::nodes::NodeFrame) cart `Rc` keeps live across the step
/// — the liveness witness the open is bounded by. Unlike the `Copy` value / contract carriers the
/// continuation is a `Box<dyn FnOnce>` consumed once, so the family is not `Copy` and the open
/// consumes the erased carrier by value. Layout-invariant: `NodeContinuation<'r>` is a `Box<dyn …>`
/// fat pointer whose representation never depends on `'r`.
pub(in crate::machine::execute) struct ContinuationFamily;

// `NodeContinuation<'r>` is one type generic only in `'r` (a boxed trait object); its fat-pointer
// layout is identical for every `'r`, so the shared `reattachable!` macro discharges the obligation.
reattachable!(ContinuationFamily => NodeContinuation<'r>);

/// Relocate a dep terminal into the consumer scope's region, retaining a surviving closure / module
/// borrow on the consumer frame. The consumer-pull lift delivers each dep un-relocated (read at the
/// step brand from its producer slot); a value-copy finish calls this to copy the value into its own
/// region so it dies with the consumer, while [`reached_frame`] retention keeps a relocated closure's
/// defining region alive past the producer's frame drop. The construction inversion's `transfer_into`
/// fold relocates instead, naming every reached region on the carrier.
fn relocate_dep_into_consumer<'b>(view: &SchedulerView<'b, '_>, value: Carried<'b>) -> Carried<'b> {
    let relocated = relocate_carried(value, view.current_scope().region);
    if let (Some(home), Some(reached)) = (
        view.current_scope().region_owner().upgrade(),
        reached_frame(relocated),
    ) {
        home.retain(reached);
    }
    relocated
}

/// Dep-finish continuation: short-circuit on the first errored dep (labelled with `dep_error_frame`),
/// else relocate each resolved dep into the consumer region and hand the values to a value-only
/// [`DepFinish`]. The short-circuit and the per-dep relocation are this combinator's job, so the node
/// stays uniform.
pub(in crate::machine::execute) fn short_circuit<'a>(
    dep_error_frame: Option<TraceFrame>,
    finish: DepFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _idx| {
        let mut values: Vec<Carried<'_>> = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(t) => values.push(relocate_dep_into_consumer(view, t.value)),
                Err(e) => {
                    return Outcome::Done(Err(propagate_dep_error(e, dep_error_frame.clone())))
                }
            }
        }
        finish(view, &values)
    })
}

/// Host-side closure for a **witnessed** dep-finish — the construction-inversion analog of
/// [`DepFinish`]. Receives the resolved dep terminals (value + reach, un-relocated) and folds them
/// — together with the finish's captured static-cell carriers — into the aggregate's witnessed
/// carrier, so the result names every region it reaches by construction. Returns `Result` so a finish
/// that hits a shape error (a non-scalar dict key) short-circuits to [`Outcome::Done`].
pub(in crate::machine::execute) type WitnessedDepFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            &[&DepTerminal<'a>],
        ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError>
        + 'a,
>;

/// Witnessed dep-finish continuation: short-circuit on the first errored dep, else hand the resolved
/// dep terminals (un-relocated, value + reach) to a [`WitnessedDepFinish`] that folds them into a
/// witnessed aggregate carrier. The fold relocates each dep once (`transfer_into`) and names the union
/// of their reaches on the carrier, so neither per-dep relocation nor `reached_frame` retention runs
/// on this path. A finish error becomes a bare [`Outcome::Done`] error.
pub(in crate::machine::execute) fn short_circuit_witnessed<'a>(
    dep_error_frame: Option<TraceFrame>,
    finish: WitnessedDepFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _idx| {
        let mut deps: Vec<&DepTerminal<'_>> = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(t) => deps.push(t),
                Err(e) => {
                    return Outcome::Done(Err(propagate_dep_error(e, dep_error_frame.clone())))
                }
            }
        }
        match finish(view, &deps) {
            Ok(carrier) => Outcome::DoneWitnessed(carrier),
            Err(e) => Outcome::Done(Err(e)),
        }
    })
}

/// Catch continuation: hand the single watched dep's terminal (Value or Err) to a [`CatchFinish`]
/// without short-circuiting, so the closure can recover or re-raise.
pub(in crate::machine::execute) fn catch_continuation<'a>(
    finish: CatchFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _idx| {
        let result = match &results[0] {
            // Relocate the watched terminal into the consumer region (the lift delivers it
            // un-relocated), so the recovered value outlives the watched producer's frame.
            Ok(t) => Ok(relocate_dep_into_consumer(view, t.value).object()),
            // Frameless: the recovery-site dispatch attaches its own frame.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        finish(view, result)
    })
}

/// Dispatch-decide continuation: a [`ResumeFn`] takes no dep values (it reads the view and spawns /
/// re-resolves), so its deps are park-only and the results slice is ignored.
pub(in crate::machine::execute) fn ignore_results<'a>(
    resume: ResumeFn<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, _results, idx| resume(view, idx))
}

#[cfg(test)]
mod erased_continuation_tests {
    //! Miri coverage for the [`ContinuationFamily`] continuation erasure: the test pins the
    //! erase → open → invoke round-trip (`Erased::erase` + the consuming externally-witnessed
    //! [`SealedExtern::open`]) under tree borrows; logical assertions are minimal — it fails when Miri
    //! reports UB, not on values.

    use super::*;
    use crate::builtins::default_scope;
    use crate::machine::core::{CallFrame, FrameStorage};
    use crate::scheduler::{Erased, Scheduler};
    use crate::witnessed::SealedExtern;
    use std::rc::Rc;

    /// A continuation capturing cart-ancestor data (a `&KObject` in the run region — a strict
    /// ancestor of the cart) is erased to `'static`, **opened** against the cart `Rc` at a rank-2
    /// brand, and *invoked* inside it, so tree borrows checks the capture read through the
    /// lifetime-fabricated box. A boxed `dyn FnOnce` is opened by value (the consuming verb) and its
    /// captured-environment read rides the fabricated brand `'b`. The cart's `outer` chain pins the
    /// ancestor region, so the step-scale fabrication is honest. Mirrors the run-loop step's
    /// continuation open + single-shot call (`run_step`); fails on UB, not values.
    #[test]
    fn erased_continuation_open_roundtrip() {
        let region = FrameStorage::run_root();
        let scope = default_scope(&region, Box::new(std::io::sink()));
        // The captured value lives in the run region — the ancestor the cart's `outer` chain pins.
        let captured: &KObject = region.region().alloc_object(KObject::Number(7.0));
        // The cart `Rc` held live to the end of the test witnesses the open below.
        let cart = Rc::new(CallFrame::new_test(scope, None));

        let continuation: NodeContinuation = Box::new(move |_view, _results, _idx| {
            // Read the run-lived capture through the reattached box.
            assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
            Outcome::Done(Err(KError::new(crate::machine::KErrorKind::ShapeError(
                "ran".to_string(),
            ))))
        });
        let erased: Erased<ContinuationFamily> = Erased::erase(continuation);
        let sched = Scheduler::new();
        let ambient = crate::machine::execute::ambient::AmbientContext::default();
        // Open the continuation against the held cart `Rc` at the brand and run the single shot inside
        // it — the same consuming externally-witnessed open the driver uses in `run_step`. The branded
        // `Outcome` is consumed in place; nothing leaves the brand.
        SealedExtern::seal(erased).open(&cart, |continuation| {
            let view = SchedulerView::new(&sched, &ambient);
            let out = continuation(&view, &[], 0);
            assert!(matches!(out, Outcome::Done(Err(_))));
        });
        // Mutate the region through a sibling pointer after the brand to catch a stacked-borrow regression.
        let _other = region.region().alloc_object(KObject::Number(8.0));
        assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
    }
}

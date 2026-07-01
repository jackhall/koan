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
//! - [`Outcome::Done`] — the node dies, producing a witnessed value or an error.
//! - [`Outcome::Continue`] — the node lives; replace its work and run again immediately (no park).
//!   A resolved call folds into this: the producer installs the per-call cart (its frame placement)
//!   and the work re-decides via the folded `invoke` / re-resolve closure on the next pop — so the
//!   dispatch→execution hand-off is a dep-free `Continue`, not a distinct trigger.
//! - [`Outcome::ParkThenContinue`] — park on deps; on resolve run a [`Continuation`] that yields
//!   another outcome.
//! - [`Outcome::Forward`] — splice the slot out as an alias of an existing producer.

use crate::machine::core::kfunction::action::{BlockEntry, CatchOk, Dep, FramePlacement};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::model::values::{Carried, CarriedFamily};

use crate::machine::{FrameSet, KError, NodeId, TraceFrame};
use crate::witnessed::reattachable;
use crate::witnessed::{Sealed, Witnessed};

use super::dispatch::{propagate_dep_error, DepRequest, ResumeFn, SchedulerView};
use super::lift::relocate_carried;
use super::nodes::NodeWork;
use super::runtime::KoanWorkload;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload), mirroring `NodeStep::Replace`; boxing the hot
// continuation path to balance variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'step> {
    /// The node dies with a value or an error. The `Ok` value is a
    /// [`Witnessed`](crate::witnessed::Witnessed) carrier already naming every region it reaches —
    /// built inside its witness closure (`yoke` / `merge` / the aggregate fold at the alloc site, a
    /// `seal_value` / `resident_*_carrier`) so `finalize` seals it without an asserted-co-location
    /// bundle. The sole value terminal for **both** channels (object and type); an error carries no
    /// value and rides the `Err` arm. The carrier is lifetime-free, so this arm carries no `'step`.
    Done(Result<Witnessed<CarriedFamily, FrameSet>, KError>),
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
        block_entry: BlockEntry<'step>,
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

#[cfg(test)]
impl<'step> Outcome<'step> {
    /// Seal a **region-pure** bare value as a `Done` terminal — the test-only constructor for a
    /// marker object that references no foreign region ([`Witnessed::resident`] fixes the empty
    /// witness). Production never mints a bare terminal: a real value is always built witnessed at its
    /// alloc site, so this stays behind `cfg(test)`.
    pub(in crate::machine::execute) fn done_resident(value: Carried<'step>) -> Self {
        Outcome::Done(Ok(Witnessed::resident(value)))
    }
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
/// - `FinishWitnessed` is the construction-inversion sibling of `Finish`: it short-circuits the same
///   way but hands the resolved dep *terminals* (value + reach) to a [`WitnessedDepFinish`] that folds
///   them into a single witnessed carrier ([`short_circuit_witnessed`]), sealing the slot as
///   [`Outcome::Done(Ok)`](Outcome::Done). The decide-side twin of the apply-side
///   `submit_dep_finish_witnessed_in_own_scope` — used by a construction decide (newtype / tagged
///   union) that parks on its value deps and builds the wrapped value naming every region it reaches.
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
    FinishWitnessed(WitnessedDepFinish<'step>),
    Catch {
        watched: Dep<'step>,
        finish: CatchFinish<'step>,
    },
    Resume {
        carrier: Option<String>,
        resume: ResumeFn<'step>,
    },
}

/// Host-side closure run by a dep-finish [`NodeWork`](super::nodes::NodeWork) once its deps resolve
/// without error. Receives the dep **values** in submission order as [`Carried`] (relocated into the
/// consumer region; an object or a type flowing in the type channel) **and**, in the same order, a
/// borrow of each dep's own [`Sealed`] carrier (un-relocated, naming the dep's reach) — so a finish
/// that commits a call threads each arg's carrier on to the body. Static elements are captured in the
/// closure. A value-consuming finish calls `.object()` on each value; a type-resolving dep arrives as
/// [`Carried::Type`]; a finish that needs no carriers ignores the slice. The finish decides against a
/// read-only [`SchedulerView`] and returns an [`Outcome`] the harness applies — it issues no graph
/// write of its own.
pub(in crate::machine::execute) type DepFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            &[Carried<'a>],
            &[&Sealed<CarriedFamily, FrameSet>],
        ) -> Outcome<'a>
        + 'a,
>;

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
    dyn for<'view> FnOnce(&SchedulerView<'a, 'view>, Result<CatchOk<'a>, KError>) -> Outcome<'a>
        + 'a,
>;

/// A resolved dep terminal as the continuation receives it, **un-relocated**. It holds the producer
/// slot's own [`Sealed`] carrier (a [`duplicate`](crate::witnessed::Sealed::duplicate) — the producer
/// keeps its terminal for other consumers), so a **construction finish** folds the dep *witnessed* via
/// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into), its reach named on the carrier
/// by construction (the [`alloc` construction
/// inversion](../../../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)). `value` is the same
/// value re-anchored **live at the step brand** (read out of the producer slot, pinned by the step
/// open) for the **value-copy** finishes still on the bare channel, which [`relocate_carried`] it into
/// the consumer region (a bare structural copy: the spine is rebuilt, a surviving closure / module
/// borrow is preserved verbatim). Such a borrow stays alive without a per-relocate reach fold: its
/// reach rides the dep's own carrier, read off `carrier.witness()` and unioned into the consumer-step
/// `pin` before the open, and folded onto the scope reach-set only when the value is *bound*
/// (`let` / user-fn arg). The dep's reach is read off the carrier the same way for every dep.
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
        let mut carriers: Vec<&Sealed<CarriedFamily, FrameSet>> = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(t) => {
                    values.push(relocate_carried(t.value, view.current_scope().brand()));
                    // The dep's own carrier (un-relocated, naming its reach) rides alongside the
                    // relocated value so a call-committing finish threads it to the body. Borrowed —
                    // a finish that keeps a carrier `duplicate`s it.
                    carriers.push(&t.carrier);
                }
                Err(e) => {
                    return Outcome::Done(Err(propagate_dep_error(e, dep_error_frame.clone())))
                }
            }
        }
        finish(view, &values, &carriers)
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
/// of their reaches on the carrier, so no separate per-dep relocation runs on this path. A finish
/// error becomes a bare [`Outcome::Done`] error.
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
            Ok(carrier) => Outcome::Done(Ok(carrier)),
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
            // Relocate the watched value into the consumer region (the lift delivers it un-relocated)
            // for a value-reading finish (TRY-WITH's `it` bind), and hand the producer's own carrier
            // alongside for a witnessed finish (CATCH folds it via `transfer_into`).
            Ok(t) => Ok(CatchOk {
                value: relocate_carried(t.value, view.current_scope().brand()),
                carrier: t.carrier.duplicate(),
            }),
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
    use crate::machine::model::KObject;
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
        let captured: &KObject = region.brand().alloc_object(KObject::Number(7.0));
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
        // Open the continuation and a scope carrier against the held cart `Rc` at the brand and run the
        // single shot inside it — the same consuming externally-witnessed open the driver uses in
        // `run_step`, where the scope is zipped in so the view reads it at the brand. The branded
        // `Outcome` is consumed in place; nothing leaves the brand.
        let scope_carrier = cart.scope_sealed();
        SealedExtern::seal(erased)
            .zip(scope_carrier)
            .open(&cart, |(continuation, scope)| {
                let view = SchedulerView::new(&sched, &ambient, scope);
                let out = continuation(&view, &[], 0);
                assert!(matches!(out, Outcome::Done(Err(_))));
            });
        // Mutate the region through a sibling pointer after the brand to catch a stacked-borrow regression.
        let _other = region.brand().alloc_object(KObject::Number(8.0));
        assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
    }
}

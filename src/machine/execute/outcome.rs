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
//! - [`Outcome::ParkThenContinue`] — park on deps; on resolve run a [`Continuation`] that yields
//!   another outcome.
//! - [`Outcome::Forward`] — splice the slot out as an alias of an existing producer.

use crate::machine::core::kfunction::action::{BlockEntry, FramePlacement};
#[cfg(test)]
use crate::machine::model::values::Carried;
use crate::machine::DeliveredCarried;

use crate::machine::{KError, NodeId, TraceFrame};
use crate::scheduler::{DepResults, Deps};
use crate::witnessed::reattachable;
#[cfg(test)]
use crate::witnessed::Witnessed;

use super::dispatch::{propagate_dep_error, DepRequest, ResumeFn, SchedulerView};
use super::nodes::{ChainOp, NodeWork};
use super::runtime::KoanWorkload;
use super::StepCarried;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the tail-call payload),
// mirroring `NodeStep::Replace`; boxing the hot continuation path to balance variants is the wrong
// trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'step> {
    /// The node dies with a value or an error. The `Ok` carrier already names every region it reaches
    /// (built inside its witness closure) so `finalize` seals it without an asserted-co-location
    /// bundle. The sole value terminal for both channels (object and type); it rides the step brand
    /// `'step` as a [`StepCarried`], confined to the step until finalize's seal exit.
    Done(Result<StepCarried<'step>, KError>),
    /// The node lives: install `work` and run again immediately (no park). `frame` rotates the
    /// per-call cart; `chain` is the pre-decided lexical-chain reshape (decided at the construction
    /// site while the contract variant is still live) and `block_entry` names any overlay scope the
    /// tail installs. A body's non-tail (leading) statements are NOT carried here — a producer with
    /// leading statements parks on them as owned deps (a [`DepRequest::BodyBlock`]) and emits this
    /// `Continue` only from the resolving finish, restoring frame uniqueness for TCO reuse. The
    /// slot's declared-return obligation does not ride here — it is wrapped onto `work`'s
    /// continuation at the construction site (see
    /// [`with_obligation`](super::obligation::with_obligation)).
    Continue {
        work: NodeWork<KoanWorkload>,
        frame: FramePlacement<'step>,
        chain: ChainOp,
        block_entry: BlockEntry<'step>,
    },
    /// Park the slot on `deps` and run `continuation` when they resolve. A dep is either a park
    /// (`Notify` edge, kept alive) or an owned entry (realizes to a harness-owned sub-slot,
    /// cascade-freed); a [`Continuation::Resume`]'s deps are all parks. `dep_error_frame` labels the
    /// dep-error short-circuit that runs before the finish.
    ParkThenContinue {
        deps: Deps<DepRequest<'step>>,
        continuation: Continuation<'step>,
        dep_error_frame: Option<TraceFrame>,
    },
    /// The slot's result *is* `producer`'s result (a bare name resolving to a binding): the harness
    /// splices the slot out rather than installing a forwarding node — finalizing directly if
    /// `producer` is ready, else aliasing the slot onto `producer` and moving its consumers to
    /// `producer`'s notify list. The single-producer invariant holds with no duplicate forwarding slot.
    Forward(NodeId),
}

#[cfg(test)]
impl<'step> Outcome<'step> {
    /// Seal a region-pure bare value as a `Done` terminal ([`Witnessed::resident`] fixes the empty
    /// witness, [`StepCarried::born`] brands it at the step). Test-only: production always builds a
    /// value witnessed at its alloc site, never bare.
    pub(in crate::machine::execute) fn done_resident(value: Carried<'step>) -> Self {
        Outcome::Done(Ok(StepCarried::born(Witnessed::resident(value))))
    }
}

/// What a [`Outcome::ParkThenContinue`] runs once its deps resolve — the closed set of "what happens
/// on wake":
/// - `FinishTerminal` hands the resolved dep terminals (un-relocated value + reach carrier) to a
///   [`TerminalDepFinish`] after the [`short_circuit`] dep-error gate; the finish returns another
///   [`Outcome`] (it may re-park). Covers both a dispatch decide's re-park/splice and the
///   action-harness / literal dep-finishes.
/// - `FinishWitnessed` folds the same terminals into a single witnessed carrier via a
///   [`WitnessedDepFinish`] (the [`seal_witnessed`] projection), sealing the slot as
///   [`Outcome::Done(Ok)`](Outcome::Done). The decide-side twin of the apply-side
///   `submit_dep_finish_witnessed_in_own_scope`, used by a construction decide (newtype / tagged
///   union) building the wrapped value naming every region it reaches.
/// - `Catch` watches the realized `watched` dep (harness-owned producer) and hands its terminal to a
///   [`CatchFinish`] without short-circuiting.
/// - `Resume` re-runs the parked dispatch decide through the [`ResumeFn`] the parking decide
///   captured; `carrier` is the parked expression's rendered summary for the deadlock report (`None`
///   when it has no renderable form).
///
/// (A bare-name forward is not a continuation — it splices out via [`Outcome::Forward`], never parking.)
pub(in crate::machine::execute) enum Continuation<'step> {
    /// Reads the resolved dep terminals directly (un-relocated value + reach carrier) and returns the
    /// next [`Outcome`]. A finish whose value must outlive the resolving step folds the dep's carrier
    /// via [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into).
    FinishTerminal(TerminalDepFinish<'step>),
    FinishWitnessed(WitnessedDepFinish<'step>),
    Catch {
        watched: DepRequest<'step>,
        finish: CatchFinish<'step>,
    },
    Resume {
        carrier: Option<String>,
        resume: ResumeFn<'step>,
    },
}

/// The fallback error-frame label for the frameless dep-finish paths (an action-harness combine or a
/// literal builder). A dispatch finish carries the consuming call's own frame instead.
pub(in crate::machine::execute) fn dep_error_frame() -> TraceFrame {
    TraceFrame::bare("<deps>", "deps")
}

/// The envelope builder — the sole production constructor of an [`Outcome::ParkThenContinue`]
/// carrying a [`Continuation::FinishTerminal`] / [`Continuation::FinishWitnessed`]. The finish is
/// wrapped in the [`short_circuit`] dep-error gate so it never observes an errored dep. `error_frame`
/// labels the propagated error; skipping it propagates frameless. (`Resume` / `Catch` continuations
/// are built at their own sites.)
pub(in crate::machine::execute) struct Await<'step> {
    deps: Deps<DepRequest<'step>>,
    dep_error_frame: Option<TraceFrame>,
}

impl<'step> Await<'step> {
    pub(in crate::machine::execute) fn on(deps: Deps<DepRequest<'step>>) -> Self {
        Await {
            deps,
            dep_error_frame: None,
        }
    }

    pub(in crate::machine::execute) fn error_frame(
        mut self,
        frame: impl Into<Option<TraceFrame>>,
    ) -> Self {
        self.dep_error_frame = frame.into();
        self
    }

    /// Seal the envelope over a witnessed finish (dep terminals folded into one witnessed carrier).
    pub(in crate::machine::execute) fn finish_witnessed(
        self,
        finish: WitnessedDepFinish<'step>,
    ) -> Outcome<'step> {
        Outcome::ParkThenContinue {
            deps: self.deps,
            continuation: Continuation::FinishWitnessed(finish),
            dep_error_frame: self.dep_error_frame,
        }
    }

    /// Seal the envelope over a terminal finish (dep terminals in, [`Outcome`] out).
    pub(in crate::machine::execute) fn finish_terminal(
        self,
        finish: TerminalDepFinish<'step>,
    ) -> Outcome<'step> {
        Outcome::ParkThenContinue {
            deps: self.deps,
            continuation: Continuation::FinishTerminal(finish),
            dep_error_frame: self.dep_error_frame,
        }
    }
}

/// Host-side closure for a catch [`NodeWork`](super::nodes::NodeWork). Receives the watched slot's
/// delivery envelope (value, reach, and retained producer pin as one unit, adopted or opened at the
/// finish's own step brand) or its error, plus a read-only view.
pub(in crate::machine::execute) type CatchFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            Result<DeliveredCarried, KError>,
        ) -> Outcome<'a>
        + 'a,
>;

/// The resolved dep terminal (value + reach carrier, un-relocated) both finishes read — defined in
/// core so the builtin-`Action` currency can name it, re-exported here. Its `value` is re-anchored
/// live at the step brand; its reach rides the dep's own `carrier`, folded onto the scope reach-set
/// only when the value is *bound* (`let` / user-fn arg).
pub(in crate::machine::execute) use crate::machine::core::kfunction::action::DepTerminal;

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries. Receives the dep terminals in submission order as
/// `Result`s (an errored dep is *not* short-circuited here — the continuation decides), the view, and
/// the slot's own index, and returns an [`Outcome`]. The combinators below build the per-family
/// behavior into the closure so the node itself never branches.
pub(in crate::machine::execute) type NodeContinuation<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            DepResults<'_, Result<DepTerminal<'a>, KError>>,
            usize,
        ) -> Outcome<'a>
        + 'a,
>;

/// `Reattachable` family for the [`NodeContinuation`] — stored erased (`Erased<ContinuationFamily>`)
/// on a lifetime-free node and opened once per step via the consuming externally-witnessed
/// [`SealedExtern::open`](crate::witnessed::SealedExtern::open). The continuation captures run-lived
/// data (the parked AST, a finish closure's captured scope) in the run region or a strict ancestor of
/// the slot's per-call cart, which the node's [`NodeFrame`](super::nodes::NodeFrame) cart `Rc` keeps
/// live across the step — the liveness witness the open is bounded by. It is a `Box<dyn FnOnce>`
/// consumed once, so the family is not `Copy` and the open consumes the erased carrier by value.
/// Layout-invariant: `NodeContinuation<'r>` is a fat pointer whose representation never depends on `'r`.
pub(in crate::machine::execute) struct ContinuationFamily;

// `NodeContinuation<'r>` is one type generic only in `'r` (a boxed trait object); its fat-pointer
// layout is identical for every `'r`, so the shared `reattachable!` macro discharges the obligation.
reattachable!(ContinuationFamily => NodeContinuation<'r>);

/// Walk the resolved dep results in delivery order, short-circuiting on the first errored dep (its
/// error propagated under `dep_error_frame`); on success return every terminal by reference in order.
fn all_or_first_error<'a, 'r>(
    results: &DepResults<'r, Result<DepTerminal<'a>, KError>>,
    dep_error_frame: &Option<TraceFrame>,
) -> Result<Vec<&'r DepTerminal<'a>>, KError> {
    let mut terminals = Vec::with_capacity(results.len());
    for r in results.all() {
        match r {
            Ok(t) => terminals.push(t),
            Err(e) => return Err(propagate_dep_error(e, dep_error_frame.clone())),
        }
    }
    Ok(terminals)
}

/// The one delivery currency a resolved dep-finish runs against: resolved dep terminals (value +
/// carrier, `[park..., owned...]` order) in, an [`Outcome`] out. A value-reading finish writes this
/// shape directly; a [`WitnessedDepFinish`] projects onto it through [`seal_witnessed`] — so
/// [`short_circuit`] is the single loop that runs either.
pub(in crate::machine::execute) type TerminalDepFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            DepResults<'_, &DepTerminal<'a>>,
        ) -> Outcome<'a>
        + 'a,
>;

/// Dep-finish continuation: short-circuit on the first errored dep (labelled with `dep_error_frame`),
/// else hand the resolved dep terminals to a [`TerminalDepFinish`]. The one delivery loop every
/// dep-finish runs through — the witnessed finish via the [`seal_witnessed`] projection.
pub(in crate::machine::execute) fn short_circuit<'a>(
    dep_error_frame: Option<TraceFrame>,
    finish: TerminalDepFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _idx| {
        let terminals = match all_or_first_error(&results, &dep_error_frame) {
            Ok(terminals) => terminals,
            Err(e) => return Outcome::Done(Err(e)),
        };
        // Re-wrap under the same park-prefix so the finish reads through one `[park..., owned...]` view.
        finish(view, results.rewrap(&terminals))
    })
}

/// Host-side closure for a witnessed dep-finish. Folds the resolved dep terminals — with the finish's
/// captured static-cell carriers — into the aggregate's witnessed carrier, so the result names every
/// region it reaches by construction. Returns `Result` so a shape error (a non-scalar dict key)
/// short-circuits to [`Outcome::Done`].
pub(in crate::machine::execute) type WitnessedDepFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'a, 'view>,
            DepResults<'_, &DepTerminal<'a>>,
        ) -> Result<StepCarried<'a>, KError>
        + 'a,
>;

/// Project a [`WitnessedDepFinish`] onto the [`TerminalDepFinish`] delivery: run the fold and seal the
/// resulting carrier (or error) as an [`Outcome::Done`]. The fold relocates each dep once
/// (`transfer_into`) and names the union of their reaches, so no separate per-dep relocation runs here.
/// The finish hands back a step-branded carrier from its own door, so it seals as-is.
pub(in crate::machine::execute) fn seal_witnessed<'a>(
    finish: WitnessedDepFinish<'a>,
) -> TerminalDepFinish<'a> {
    Box::new(move |view, terminals| match finish(view, terminals) {
        Ok(carrier) => Outcome::Done(Ok(carrier)),
        Err(e) => Outcome::Done(Err(e)),
    })
}

/// Catch continuation: hand the single watched dep's terminal (Value or Err) to a [`CatchFinish`]
/// without short-circuiting, so the closure can recover or re-raise.
pub(in crate::machine::execute) fn catch_continuation<'a>(
    finish: CatchFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _idx| {
        let result = match &results.all()[0] {
            // The watched producer's own delivery envelope, duplicated (the producer keeps its
            // terminal); the finish adopts or opens it at its own step brand.
            Ok(t) => Ok(t.delivered.duplicate()),
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
    use crate::machine::core::{run_root_storage, CallFrame, FrameStorageExt};
    use crate::machine::model::KObject;
    use crate::scheduler::{Erased, Scheduler};
    use crate::witnessed::SealedExtern;
    use std::rc::Rc;

    /// A continuation capturing cart-ancestor data (a `&KObject` in the run region) is erased to
    /// `'static`, opened by value against the cart `Rc` at a fabricated brand, and invoked inside it,
    /// so tree borrows checks the capture read through the lifetime-fabricated box. The cart's `outer`
    /// chain pins the ancestor region, so the fabrication is honest. Mirrors the run-loop step's
    /// continuation open + single-shot call (`run_step`); fails on UB, not values.
    #[test]
    fn erased_continuation_open_roundtrip() {
        let region = run_root_storage();
        let scope = default_scope(&region, Box::new(std::io::sink()));
        // The captured value lives in the run region — the ancestor the cart's `outer` chain pins.
        let captured: &KObject = region.brand().alloc_object(KObject::Number(7.0));
        // The cart `Rc` held live to the end of the test witnesses the open below.
        let cart = Rc::new(CallFrame::new(scope));

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
        // Open the continuation and scope carrier against the held cart `Rc` and run the single shot
        // inside it — the same consuming open the driver uses in `run_step`. Nothing leaves the brand.
        let scope_carrier = cart.scope_sealed();
        SealedExtern::seal(erased)
            .zip(scope_carrier)
            .open(&cart, |(continuation, scope)| {
                let view = SchedulerView::new(&sched, &ambient, scope, cart.storage_rc());
                let empty: &[Result<DepTerminal, KError>] = &[];
                let out = continuation(&view, DepResults::new(empty, 0), 0);
                assert!(matches!(out, Outcome::Done(Err(_))));
            });
        // Mutate the region through a sibling pointer after the brand to catch a stacked-borrow regression.
        let _other = region.brand().alloc_object(KObject::Number(8.0));
        assert!(matches!(captured, KObject::Number(n) if *n == 7.0));
    }
}

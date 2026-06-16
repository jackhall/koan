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
use crate::machine::model::values::{Carried, KObject};
use crate::machine::{KError, NodeId, TraceFrame};

use super::dispatch::{propagate_dep_error, DepRequest, ResumeFn, SchedulerView};
use super::nodes::NodeWork;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload), mirroring `NodeStep::Replace`; boxing the hot
// continuation path to balance variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'run, 's> {
    /// The node dies with a value or an error. The value is bound to the per-step frame lifetime
    /// `'s`, not `'run`: it is born in the node's own per-call frame (a builtin allocates it there,
    /// a forwarded dep arrives already lifted into it) and the scheduler relocates it across each
    /// dep edge — the consumer-pull lift — into the consuming node's frame. `'run: 's`.
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
        work: NodeWork<'run>,
        frame: FramePlacement<'run>,
        contract: Option<ReturnContract<'run>>,
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
        deps: Vec<DepRequest<'run>>,
        park_count: usize,
        cont: Continuation<'run>,
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
    Box<dyn for<'v> FnOnce(&SchedulerView<'a, 'v>, &[Carried<'a>]) -> Outcome<'a, 'a> + 'a>;

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
    dyn for<'v> FnOnce(&SchedulerView<'a, 'v>, Result<&'a KObject<'a>, KError>) -> Outcome<'a, 'a>
        + 'a,
>;

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries. It receives the dep terminals in submission order
/// as `Result`s (an errored dep is *not* short-circuited by the handler — the continuation decides),
/// a read-only [`SchedulerView`], and the slot's own index, and returns an [`Outcome`] the harness
/// applies. The per-family behaviors (combine short-circuit, catch recover, dispatch decide) are
/// built into the closure by the combinators below, so the node itself never branches.
pub(in crate::machine::execute) type NodeCont<'a> = Box<
    dyn for<'v, 's> FnOnce(
            &SchedulerView<'a, 'v>,
            &[Result<Carried<'s>, KError>],
            usize,
        ) -> Outcome<'a, 's>
        + 'a,
>;

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
        // The deps were pull-lifted into this node's frame; re-expose them at `'run` for the
        // concrete finish, then reattach its outcome to the step lifetime `'s`.
        shorten_outcome(finish(view, deps_for_builtin(&values)))
    })
}

/// Catch continuation: hand the single watched dep's terminal (Value or Err) to a [`CatchFinish`]
/// without short-circuiting, so the closure can recover or re-raise.
pub(in crate::machine::execute) fn catch_cont<'a>(finish: CatchFinish<'a>) -> NodeCont<'a> {
    Box::new(move |view, results, _idx| {
        let result = match &results[0] {
            // Re-expose the watched terminal at `'run` for the concrete finish (pull-lifted here).
            Ok(c) => Ok(obj_for_builtin(c.object())),
            // Frameless: the recovery-site dispatch attaches its own frame.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        shorten_outcome(finish(view, result))
    })
}

/// Dispatch-decide continuation: a [`ResumeFn`] takes no dep values (it reads the view and spawns /
/// re-resolves), so its deps are park-only and the results slice is ignored.
pub(in crate::machine::execute) fn ignore_results<'a>(resume: ResumeFn<'a>) -> NodeCont<'a> {
    Box::new(move |view, _results, idx| shorten_outcome(resume(view, idx)))
}

/// Reattach a step-bound `'s` terminal up to `'run` for storage in the slot table. The value is
/// born in the producer's per-call frame (or is genuinely `'run`-lived), and the harness pins that
/// frame's `Rc` alongside the terminal in [`SlotState::Done`](super::scheduler) until the slot is
/// freed, so the stored `'run` view cannot outlive its backing arena. The reattach is needed only
/// because `Carried` is invariant; this is the held-`Rc` re-exposure seam.
pub(in crate::machine::execute) fn pin_carried_to_run<'run>(value: Carried<'_>) -> Carried<'run> {
    // SAFETY: lifetime-only reattach; the frame `Rc` co-stored in `SlotState::Done` heap-pins the
    // backing arena for as long as the terminal is readable. See the doc comment.
    unsafe { std::mem::transmute::<Carried<'_>, Carried<'run>>(value) }
}

/// Reattach the dep terminals delivered at the step lifetime `'s` up to `'run` for the duration of
/// a synchronous builtin finish. The values were pull-lifted into the consumer's per-call frame,
/// which is heap-pinned for the whole step, so re-exposing them at `'run` across the finish call
/// cannot dangle; the reattach is needed only because `Carried` is invariant. The builtin action
/// boundary ([`AwaitContinue`](crate::machine::core::kfunction::action) / `CatchCont`) is concrete
/// in `'run`, so the deps meet it at `'run`.
pub(in crate::machine::execute) fn deps_for_builtin<'b, 'run, 's>(
    results: &'b [Carried<'s>],
) -> &'b [Carried<'run>] {
    // SAFETY: `'run: 's`; the values outlive the synchronous finish call (frame-pinned). Lifetime-
    // only reattach of an invariant carrier — see the doc comment.
    unsafe { std::mem::transmute::<&[Carried<'s>], &[Carried<'run>]>(results) }
}

/// Reattach the consumer's pull-lifted dep terminals down to the step lifetime `'s` for delivery to
/// the continuation. The lift hook returns `'run`, but each value was just copied into this
/// consumer's per-call frame and dies with it, so the honest type is `'s` (`'run: 's`); the reattach
/// is needed only because `Carried` is invariant.
pub(in crate::machine::execute) fn deps_at_step<'b, 'run, 's>(
    results: &'b [Result<Carried<'run>, KError>],
) -> &'b [Result<Carried<'s>, KError>] {
    // SAFETY: lifetime-only reattach of an invariant carrier to a shorter lifetime it genuinely
    // outlives (the values die with the consumer frame, i.e. at `'s`). See the doc comment.
    unsafe {
        std::mem::transmute::<&'b [Result<Carried<'run>, KError>], &'b [Result<Carried<'s>, KError>]>(
            results,
        )
    }
}

/// The single-object dual of [`deps_for_builtin`] for a catch's watched terminal. Same soundness
/// (frame-pinned across the synchronous finish call); reattach needed only for `KObject`'s invariance.
pub(in crate::machine::execute) fn obj_for_builtin<'run>(obj: &KObject<'_>) -> &'run KObject<'run> {
    // SAFETY: `'run: 's`; the watched value outlives the synchronous finish call (frame-pinned).
    unsafe { std::mem::transmute::<&KObject<'_>, &'run KObject<'run>>(obj) }
}

/// Reattach an `Outcome`'s output lifetime from `'run` down to a per-step `'s` (`'run: 's`). A
/// decide composes at `Outcome<'run, 'run>`; a continuation that returns a decide's outcome must
/// hand it back at the step lifetime `'s` the continuation is parameterized over.
///
/// Sound because only the `Done(Carried<'s>)` arm carries an `'s`-typed payload, and that payload
/// is genuinely `'run`-lived (born in the producer frame, which outlives the step), so viewing it
/// at the shorter `'s` cannot dangle; the reattach is needed only because [`Outcome`] is invariant
/// in `'s`. The other arms carry `'run` work untouched.
pub(in crate::machine::execute) fn shorten_outcome<'run, 's>(
    outcome: Outcome<'run, 'run>,
) -> Outcome<'run, 's> {
    // SAFETY: lifetime-only reattach of an invariant carrier whose `'s` payload outlives `'s`; see
    // the doc comment. Same shape as the arena re-exposure transmutes the per-call protocol uses.
    unsafe { std::mem::transmute::<Outcome<'run, 'run>, Outcome<'run, 's>>(outcome) }
}

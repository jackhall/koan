//! The unified scheduler-step currency.
//!
//! Every node step — a fresh dispatch decide, a finish, a builtin body, an invoke — decides
//! against a read-only [`DecideCtx`](super::decide::DecideCtx) and **returns** an [`Outcome`];
//! the harness's apply ([`super::harness`]) turns an outcome into the scheduler-graph writes it
//! implies and the [`StepVerdict`](crate::scheduler::StepVerdict) the drain applies. The scheduler never learns *what* a step ran (dispatch / invoke / builtin) nor
//! *whether* it ran before — only a read view in and an outcome out.
//!
//! The taxonomy is AST-free — no variant names a `KFunction` or a `KExpression`:
//! - [`Outcome::Done`] — the node dies, producing a witnessed value or an error.
//! - [`Outcome::Continue`] — the node lives; replace its work and run again immediately (no park).
//! - [`Outcome::Park`] — park on deps; on resolve run a [`Continuation`] that yields another
//!   outcome.
//! - [`Outcome::Forward`] — splice the slot out as an alias of the producer an edge names.

use crate::machine::DeliveredCarried;
use crate::machine::core::{BlockEntry, FramePlacement, ReturnContract, ScopeId};
#[cfg(test)]
use crate::machine::model::Carried;
use crate::machine::model::WorkingExpression;

#[cfg(test)]
use crate::machine::Scope;
use crate::machine::{KError, NodeId, TraceFrame};
use crate::scheduler::Deps;
use crate::scheduler::EdgeId;
use crate::witnessed::reattachable;

use super::StepCarried;
use super::decide::{DecideCtx, DepRequest, ResumeFn, propagate_dep_error};
use super::harness::KoanWorkload;
use super::nodes::WorkLabel;
use super::nodes::{ChainOp, NodeWork};
use super::obligation::ReturnObligation;
use crate::machine::core::BlockRequest;

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant; boxing the hot continuation path to balance
// variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'step> {
    /// The node dies with a value or an error. The `Ok` carrier already names every region it
    /// reaches (built inside its witness closure), so `finalize` seals it without an
    /// asserted-co-location bundle. The sole value terminal for both channels (object and type).
    Done(Result<StepCarried<'step>, KError>),
    /// The node lives: install `work` and run again immediately (no park). `chain` is pre-decided at
    /// the construction site, while the contract variant is still live.
    ///
    /// A body's non-tail (leading) statements are NOT carried here — a producer with leading
    /// statements waits on them as deps (a [`BlockRequest::Body`]) and emits this `Continue` only
    /// from the resolving finish, restoring frame uniqueness for TCO reuse. The slot's
    /// declared-return obligation likewise does not ride here: it is wrapped onto `work`'s
    /// continuation at the construction site
    /// ([`with_obligation`](super::obligation::with_obligation)).
    Continue {
        work: NodeWork<'step, KoanWorkload>,
        frame: FramePlacement,
        chain: ChainOp,
        block_entry: BlockEntry<'step>,
        /// What the next incarnation renders as if the drain deadlocks on it. Read only by the
        /// arms that mint a fresh anchor; a replace that keeps the slot's anchor keeps its label
        /// with it.
        label: WorkLabel,
    },
    /// Park the slot on `deps` and run `continuation` when they resolve. `dep_error_frame` labels
    /// the dep-error short-circuit that runs before the finish.
    Park {
        deps: ParkDeps<'step>,
        continuation: Continuation<'step>,
        dep_error_frame: Option<TraceFrame>,
    },
    /// The slot's result *is* the result behind `source` (a bare name resolving to a binding): the
    /// harness splices the slot out rather than installing a forwarding node, so the
    /// single-producer invariant holds with no duplicate forwarding slot.
    Forward(EdgeId),
}

#[cfg(test)]
impl<'step> Outcome<'step> {
    /// Seal a region-pure bare value as a `Done` terminal. Test-only: production always builds a
    /// value witnessed at its alloc site, never bare.
    pub(in crate::machine::execute) fn done_resident(
        scope: &Scope<'step>,
        value: Carried<'step>,
    ) -> Self {
        Outcome::Done(Ok(StepCarried::born(scope.resident(value))))
    }
}

/// The dep-free re-decide-in-place `Continue`: replace the slot's work and run it again in the
/// slot's current cart, scope, and chain.
pub(in crate::machine::execute) fn continue_inline(
    work: NodeWork<'_, KoanWorkload>,
    label: WorkLabel,
) -> Outcome<'_> {
    Outcome::Continue {
        work,
        frame: FramePlacement::Inherit,
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
        label,
    }
}

/// `None` for a blockless (frameless) tail.
fn block_entry_scope(block_entry: &BlockEntry<'_>) -> Option<ScopeId> {
    match block_entry {
        BlockEntry::None => None,
        BlockEntry::FrameScope(frame) => Some(frame.scope_id()),
        BlockEntry::Overlay(overlay) => Some(overlay.id),
    }
}

/// Tail-replace into `tail` under a still-live `contract`. The obligation is keep-first: the
/// chain's established obligation — deposited on `view` — wins over this call's own contract, and
/// the winner rides the replacement continuation so the next step re-deposits it. `view` is
/// whichever view the caller holds; a finish's wake-time view already sees the obligation its park
/// re-deposited.
pub(in crate::machine::execute) fn tail_continue<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    tail: WorkingExpression<'step>,
    contract: Option<ReturnContract<'step>>,
    frame: FramePlacement,
    block_entry: BlockEntry<'step>,
    body_index: usize,
) -> Outcome<'step> {
    let chain = ChainOp::decide(
        block_entry_scope(&block_entry),
        contract.as_ref(),
        body_index,
    );
    let winner = view
        .current_obligation_duplicate()
        .or_else(|| contract.map(ReturnObligation::seal));
    let label = WorkLabel::of(&tail);
    Outcome::Continue {
        work: super::decide::decide_tail(tail, winner),
        frame,
        chain,
        block_entry,
        label,
    }
}

/// What a [`Outcome::Park`] runs once its deps resolve — the closed set of "what happens on wake":
/// - `Finish` runs behind the [`short_circuit`] dep-error gate, so it never observes an errored dep.
/// - `Catch` watches one realized dep *without* short-circuiting, so the closure can recover.
/// - `Resume` re-runs the parked dispatch decide. It carries no deadlock sample of its own: a park
///   keeps the slot's anchor, so the [`WorkLabel`] minted at submission is still the one a stuck
///   slot renders through.
pub(in crate::machine::execute) enum Continuation<'step> {
    /// Reads the resolved dep terminals directly (un-relocated value + reach carrier) and returns
    /// the next [`Outcome`] — it may re-park. A finish whose value must outlive the resolving step
    /// folds the dep's carrier via
    /// [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into).
    Finish(TerminalDepFinish<'step>),
    Catch {
        watched: DepRequest<'step>,
        finish: CatchFinish<'step>,
    },
    Resume {
        resume: ResumeFn<'step>,
    },
}

/// The fallback error-frame label for the frameless dep-finish paths (an action-harness combine or a
/// literal builder). A dispatch finish carries the consuming call's own frame instead.
pub(in crate::machine::execute) fn dep_error_frame() -> TraceFrame {
    TraceFrame::bare("<deps>", "deps")
}

/// What a park waits on — the two dep-wiring doors, told apart by whether the dep count is known at
/// declaration time. The harness has one realization path per arm; nothing converts between them.
pub(in crate::machine::execute) enum ParkDeps<'step> {
    /// A dep list: one source per entry, in the builder's order. Every [`DepRequest`] realizes to
    /// exactly one producer, so a caller's [`Deps::request`] index is the position its result comes
    /// back at.
    List(Deps<DepRequest<'step>>),
    /// A statement block: one dep per statement, in declaration order. The count is only known once
    /// the block is split, so it is never mixed with named deps.
    Block(BlockRequest<'step>),
}

/// The envelope builder for an [`Outcome::Park`] carrying a [`Continuation::Finish`]. A witnessed
/// finish is projected onto the terminal delivery at construction, so the projection never rides as
/// data. Skipping `error_frame` propagates a dep error frameless.
pub(in crate::machine::execute) struct Await<'step> {
    deps: ParkDeps<'step>,
    dep_error_frame: Option<TraceFrame>,
}

impl<'step> Await<'step> {
    pub(in crate::machine::execute) fn on(deps: Deps<DepRequest<'step>>) -> Self {
        Await::on_park_deps(ParkDeps::List(deps))
    }

    pub(in crate::machine::execute) fn on_block(block: BlockRequest<'step>) -> Self {
        Await::on_park_deps(ParkDeps::Block(block))
    }

    fn on_park_deps(deps: ParkDeps<'step>) -> Self {
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

    /// Seal the envelope over a witnessed finish, applying the [`seal_witnessed`] projection here,
    /// at construction.
    pub(in crate::machine::execute) fn finish_witnessed(
        self,
        finish: WitnessedDepFinish<'step>,
    ) -> Outcome<'step> {
        self.finish_terminal(seal_witnessed(finish))
    }

    pub(in crate::machine::execute) fn finish_terminal(
        self,
        finish: TerminalDepFinish<'step>,
    ) -> Outcome<'step> {
        Outcome::Park {
            deps: self.deps,
            continuation: Continuation::Finish(finish),
            dep_error_frame: self.dep_error_frame,
        }
    }
}

/// Host-side closure for a catch [`NodeWork`](super::nodes::NodeWork). The watched slot's delivery
/// envelope carries value, reach, and retained producer pin as one unit, adopted or opened at the
/// finish's own step brand.
pub(in crate::machine::execute) type CatchFinish<'a> = Box<
    dyn for<'view> FnOnce(
            &DecideCtx<'_, 'a, 'view>,
            Result<DeliveredCarried, KError>,
        ) -> Outcome<'a>
        + 'a,
>;

/// The resolved dep terminal every finish reads — a delivered resident of a region this step
/// already covers, re-branded once at step start. Lives in core so the builtin-`Action` currency
/// can name it.
pub(in crate::machine::execute) use crate::machine::core::DepTerminal;

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries. Dep terminals arrive in submission order, and an
/// errored dep is *not* short-circuited here: the continuation decides. The combinators below build
/// the per-family behavior into the closure so the node itself never branches.
pub(in crate::machine::execute) type NodeContinuation<'a> = Box<
    dyn for<'view, 'd> FnOnce(
            &DecideCtx<'_, 'a, 'view>,
            &[Result<DepTerminal<'d>, KError>],
            NodeId,
        ) -> Outcome<'a>
        + 'a,
>;

/// `Reattachable` family for the [`NodeContinuation`]: a one-shot `Box<dyn FnOnce>` owning its
/// captures, so it takes the `droppable` arm and rests as
/// `SealedPinned<ContinuationFamily, Rc<SlotFrame>>`, sealed against the slot's anchor at install.
/// That anchor is what covers the run-lived data the continuation captures — the parked AST, a
/// finish closure's captured scope, held in the run region or a strict ancestor of the slot's
/// per-call cart — for the whole dormant life, which is the owned tier's standing obligation
/// ([workgraph/design/witnessed-memory.md § What a droppable family
/// accepts](../../../workgraph/design/witnessed-memory.md#what-a-droppable-family-accepts)).
pub(in crate::machine::execute) struct ContinuationFamily;

reattachable!(droppable ContinuationFamily => NodeContinuation<'r>);

fn all_or_first_error<'r, 'd>(
    results: &'r [Result<DepTerminal<'d>, KError>],
    dep_error_frame: &Option<TraceFrame>,
) -> Result<Vec<&'r DepTerminal<'d>>, KError> {
    let mut terminals = Vec::with_capacity(results.len());
    for r in results {
        match r {
            Ok(t) => terminals.push(t),
            Err(e) => return Err(propagate_dep_error(e, dep_error_frame.clone())),
        }
    }
    Ok(terminals)
}

/// The one delivery currency a resolved dep-finish runs against. A value-reading finish writes this
/// shape directly; a [`WitnessedDepFinish`] projects onto it through [`seal_witnessed`] — so
/// [`short_circuit`] is the single loop that runs either.
pub(in crate::machine::execute) type TerminalDepFinish<'a> = Box<
    dyn for<'view, 'd> FnOnce(&DecideCtx<'_, 'a, 'view>, &[&DepTerminal<'d>]) -> Outcome<'a> + 'a,
>;

/// Dep-finish continuation: short-circuit on the first errored dep (labelled with
/// `dep_error_frame`), else hand the resolved dep terminals to `finish`. The one delivery loop
/// every dep-finish runs through.
pub(in crate::machine::execute) fn short_circuit<'a>(
    dep_error_frame: Option<TraceFrame>,
    finish: TerminalDepFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _id| {
        let terminals = match all_or_first_error(results, &dep_error_frame) {
            Ok(terminals) => terminals,
            Err(e) => return Outcome::Done(Err(e)),
        };
        finish(view, &terminals)
    })
}

/// Host-side closure for a witnessed dep-finish. Folds the resolved dep terminals — with the
/// finish's captured static-cell carriers — into the aggregate's witnessed carrier, so the result
/// names every region it reaches by construction. Returns `Result` so a shape error (a non-scalar
/// dict key) short-circuits to [`Outcome::Done`].
pub(in crate::machine::execute) type WitnessedDepFinish<'a> = Box<
    dyn for<'view, 'd> FnOnce(
            &DecideCtx<'_, 'a, 'view>,
            &[&DepTerminal<'d>],
        ) -> Result<StepCarried<'a>, KError>
        + 'a,
>;

/// Project a [`WitnessedDepFinish`] onto the [`TerminalDepFinish`] delivery. The fold relocates each
/// dep once (`transfer_into`) and names the union of their reaches, so no separate per-dep
/// relocation runs here, and it hands back a step-branded carrier that seals as-is.
pub(in crate::machine::execute) fn seal_witnessed<'a>(
    finish: WitnessedDepFinish<'a>,
) -> TerminalDepFinish<'a> {
    Box::new(move |view, terminals| match finish(view, terminals) {
        Ok(carrier) => Outcome::Done(Ok(carrier)),
        Err(e) => Outcome::Done(Err(e)),
    })
}

/// Catch continuation: no short-circuit on an errored dep, so the closure can recover or re-raise.
pub(in crate::machine::execute) fn catch_continuation<'a>(
    finish: CatchFinish<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, results, _id| {
        let result = match &results[0] {
            // The watched producer's delivered resident, lifted back into an envelope owning its
            // whole reach — the finish adopts or opens it at its own step brand.
            Ok(t) => Ok(view.current_scope().lift_spliced(&t.cell)),
            // Frameless: the recovery-site dispatch attaches its own frame.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        finish(view, result)
    })
}

/// Dispatch-decide continuation: a [`ResumeFn`] takes no dep values (it reads the view and spawns /
/// re-resolves), so the results slice is ignored.
pub(in crate::machine::execute) fn ignore_results<'a>(
    resume: ResumeFn<'a>,
) -> NodeContinuation<'a> {
    Box::new(move |view, _results, id| resume(view, id))
}

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
use crate::machine::core::resolve_location;
use crate::machine::core::{
    BlockEntry, CallFrame, FramePlacement, FrameStorageExt, RegionBrand, ReturnContract, ScopeId,
};
#[cfg(test)]
use crate::machine::model::Carried;
use crate::machine::model::WorkingExpression;
use crate::machine::model::{KType, RunRegistries};
use crate::source::SourceRef;

#[cfg(test)]
use crate::machine::Scope;
use crate::machine::{KError, NodeId, TraceFrame};
use crate::scheduler::Deps;
use crate::scheduler::EdgeId;
use crate::witnessed::erase_to_static;
use crate::witnessed::reattachable;
use crate::witnessed::{BumpAllocator, BumpVec};
use std::rc::Rc;

use super::StepCarried;
use super::decide::{DecideCtx, DepRequest, propagate_dep_error};
use super::harness::KoanWorkload;
use super::nodes::WorkLabel;
use super::nodes::{ChainOp, NodeWork};
use super::obligation::{ParkState, ReturnObligation};
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
    /// The node lives: install `replacement`'s work under its frame placement and run again
    /// immediately (no park). `chain` is pre-decided at the construction site, while the contract
    /// variant is still live.
    ///
    /// A body's non-tail (leading) statements are NOT carried here — a producer with leading
    /// statements waits on them as deps (a [`BlockRequest::Body`]) and emits this `Continue` only
    /// from the resolving finish, restoring frame uniqueness for TCO reuse. The slot's
    /// declared-return obligation likewise does not ride here: it is set on `work`'s
    /// [`NodeContinuation`] at the construction site, and the next step deposits it.
    Continue {
        replacement: Replacement,
        chain: ChainOp,
        block_entry: BlockEntry<'step>,
        /// What the next incarnation renders as if the drain deadlocks on it. Read only by the
        /// arms that mint a fresh anchor; a replace that keeps the slot's anchor keeps its label
        /// with it.
        label: WorkLabel,
    },
    /// Park the slot on `deps` and run `continuation` when they resolve. `dep_error_frame` labels
    /// the dep-error short-circuit that runs before the finish, rendering only if one fires.
    Park {
        deps: ParkDeps<'step>,
        continuation: Continuation<'step>,
        dep_error_frame: Option<DeferredTraceFrame<'step>>,
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

/// The replacement a [`Outcome::Continue`] installs: the frame placement and the work it hosts,
/// coupled so a bumped closure's host region is definitionally the region of the frame the work
/// installs under. The fields are private and the constructors below are the only doors; the fresh
/// ones mint the host brand off the very frame the placement carries, so pairing a fresh placement
/// with work hosted in a sibling cart's region is unrepresentable at a construction site.
///
/// The work rides **pre-erased**: a fresh constructor's host brand borrows the frame `Rc` that then
/// moves into the placement, so the erase is what ends that borrow. The erased work is stored,
/// never run, until the drain seals it against the slot's effective anchor — which for a fresh
/// placement is the fresh cart itself, so the seal pins the very region hosting the bytes.
pub(in crate::machine::execute) struct Replacement {
    work: NodeWork<'static, KoanWorkload>,
    frame: FramePlacement,
}

impl Replacement {
    /// Keep the slot's cart: `work` is hosted wherever the deciding step built it — the current
    /// brand, i.e. the kept cart or a strict ancestor, both of which outlive the slot.
    pub(in crate::machine::execute) fn inherit(work: NodeContinuation<'_>) -> Replacement {
        Replacement {
            work: NodeWork::new(erase_to_static::<ContinuationFamily>(work)),
            frame: FramePlacement::Inherit,
        }
    }

    /// Install `frame` as the slot's TCO tail cart and host `build`'s continuation in that cart's
    /// own region: the host brand is minted here, off the same frame the placement installs. `'f`
    /// is the caller's borrow of its frame binding, so `'step`-typed captures shorten into `build`
    /// by ordinary covariance.
    pub(in crate::machine::execute) fn fresh_tail<'f, F>(
        frame: &'f Rc<CallFrame>,
        build: F,
    ) -> Replacement
    where
        F: FnOnce(RegionBrand<'f>) -> NodeContinuation<'f>,
    {
        Replacement::fresh(
            FramePlacement::FreshTail {
                frame: Rc::clone(frame),
            },
            frame,
            build,
        )
    }

    /// [`Self::fresh_tail`]'s twin for a builtin's pre-built child cart (MATCH / TRY / EVAL).
    pub(in crate::machine::execute) fn fresh_child<'f, F>(
        frame: &'f Rc<CallFrame>,
        build: F,
    ) -> Replacement
    where
        F: FnOnce(RegionBrand<'f>) -> NodeContinuation<'f>,
    {
        Replacement::fresh(
            FramePlacement::FreshChild {
                frame: Rc::clone(frame),
            },
            frame,
            build,
        )
    }

    fn fresh<'f, F>(placement: FramePlacement, frame: &'f Rc<CallFrame>, build: F) -> Replacement
    where
        F: FnOnce(RegionBrand<'f>) -> NodeContinuation<'f>,
    {
        let work = build(frame.storage().brand());
        Replacement {
            work: NodeWork::new(erase_to_static::<ContinuationFamily>(work)),
            frame: placement,
        }
    }

    /// Decompose for the apply — the one reader.
    pub(in crate::machine::execute) fn into_parts(
        self,
    ) -> (NodeWork<'static, KoanWorkload>, FramePlacement) {
        (self.work, self.frame)
    }
}

/// The dep-free re-decide-in-place `Continue`: replace the slot's work and run it again in the
/// slot's current cart, scope, and chain.
pub(in crate::machine::execute) fn continue_inline(
    work: NodeContinuation<'_>,
    label: WorkLabel,
) -> Outcome<'_> {
    Outcome::Continue {
        replacement: Replacement::inherit(work),
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
        .current_obligation()
        .or_else(|| contract.map(ReturnObligation::seal));
    let label = WorkLabel::of(&tail);
    // The decide's closure is hosted by the region the replacement work installs under: an
    // `Inherit` tail stays in the slot's current cart, whose brand is in hand here, and a fresh
    // placement's constructor supplies the brand of the cart it installs.
    let replacement = match frame {
        FramePlacement::Inherit => Replacement::inherit(super::decide::decide_tail(
            tail,
            winner,
            view.current_scope().brand(),
        )),
        FramePlacement::FreshTail { frame } => Replacement::fresh_tail(&frame, |host| {
            super::decide::decide_tail(tail, winner, host)
        }),
        FramePlacement::FreshChild { frame } => Replacement::fresh_child(&frame, |host| {
            super::decide::decide_tail(tail, winner, host)
        }),
    };
    Outcome::Continue {
        replacement,
        chain,
        block_entry,
        label,
    }
}

/// What a [`Outcome::Park`] runs once its deps resolve. Both arms carry an already-composed,
/// already-erased [`ContinuationCall`]: the dep-error gate, the witnessed projection and the
/// decide's results-dropping adapter are folded in at the construction envelope, so the apply
/// lowers a park by handing the call straight to the replacement work.
///
/// A park carries no deadlock sample of its own: it keeps the slot's anchor, so the [`WorkLabel`]
/// minted at submission is still the one a stuck slot renders through.
pub(in crate::machine::execute) enum Continuation<'step> {
    /// Runs as soon as the wired deps resolve — a dep-finish behind its [`gated`] short-circuit, or
    /// a dep-free decide the park re-runs on wake. A finish whose value must outlive the resolving
    /// step folds the dep's carrier via
    /// [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into).
    Ready(ContinuationCall<'step>),
    /// Watches one dep, realized at apply time, *without* short-circuiting on its error, so the
    /// finish can recover.
    Catch {
        watched: DepRequest<'step>,
        finish: ContinuationCall<'step>,
    },
}

/// A dep-error frame in retained form: a `Copy` capture that renders into a [`TraceFrame`] only at
/// error-construction time, so a step that completes without a dep error allocates no trace text.
///
/// Every park holding one also holds the captured expression alive through its continuation's
/// anchor, so [`render`](Self::render) always runs while the expression's region is live.
#[derive(Clone, Copy)]
pub(in crate::machine::execute) enum DeferredTraceFrame<'step> {
    /// A scheduler-internal frame keyed off the expression the slot dispatches.
    Working {
        function: &'static str,
        expr: WorkingExpression<'step>,
    },
    /// A frame with fixed label text and no originating expression.
    Bare {
        function: &'static str,
        expression: &'static str,
    },
    /// A callable contract's frame: the call site's source extent and the callable's interned
    /// `value_ktype`. The site names *which* call is at fault, the type names the signature it
    /// broke; both are `Copy`, so a call that never errors renders neither.
    Callable {
        site: Option<SourceRef>,
        ktype: KType,
    },
}

impl DeferredTraceFrame<'_> {
    pub(in crate::machine::execute) fn render(&self, registries: &RunRegistries) -> TraceFrame {
        match self {
            Self::Working { function, expr } => {
                TraceFrame::from_expr(*function, expr, &registries.labels)
            }
            Self::Bare {
                function,
                expression,
            } => TraceFrame::bare(*function, *expression),
            Self::Callable { site, ktype } => {
                let by_name = ktype.name(registries);
                match site {
                    Some(site) => TraceFrame {
                        function: site.text(),
                        expression: by_name,
                        location: Some(resolve_location(*site)),
                    },
                    None => TraceFrame::bare(by_name.clone(), by_name),
                }
            }
        }
    }
}

/// The fallback error-frame label for the frameless dep-finish paths (an action-harness combine or a
/// literal builder). A dispatch finish carries the consuming call's own frame instead.
pub(in crate::machine::execute) fn dep_error_frame() -> DeferredTraceFrame<'static> {
    DeferredTraceFrame::Bare {
        function: "<deps>",
        expression: "deps",
    }
}

/// A park's dep list, hosted on the **step's scratch arena**: the list is built by the decide,
/// read by the wiring door, and dead by the end of the pop that produced it, so the allocator
/// parameter is what makes that confinement a compile fact rather than a convention. Every park
/// construction site therefore holds the step's own handle — `ctx.scratch()`, or the one the apply
/// harness carries.
pub(in crate::machine::execute) type StepDeps<'step> =
    Deps<DepRequest<'step>, BumpAllocator<'step>>;

/// What a park waits on — the two dep-wiring doors, told apart by whether the dep count is known at
/// declaration time. The harness has one realization path per arm; nothing converts between them.
pub(in crate::machine::execute) enum ParkDeps<'step> {
    /// A dep list: one source per entry, in the builder's order. Every [`DepRequest`] realizes to
    /// exactly one producer, so a caller's [`Deps::request`] index is the position its result comes
    /// back at.
    List(StepDeps<'step>),
    /// A statement block: one dep per statement, in declaration order. The count is only known once
    /// the block is split, so it is never mixed with named deps.
    Block(BlockRequest<'step>),
}

/// The envelope builder for an [`Outcome::Park`] carrying a dep-finish. Both the witnessed
/// projection and the dep-error gate compose here, at construction, so neither rides as data and
/// the whole finish crosses into storage through one erasure. Skipping `error_frame` propagates a
/// dep error frameless.
pub(in crate::machine::execute) struct Await<'step> {
    deps: ParkDeps<'step>,
    dep_error_frame: Option<DeferredTraceFrame<'step>>,
}

impl<'step> Await<'step> {
    pub(in crate::machine::execute) fn on(deps: StepDeps<'step>) -> Self {
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
        frame: impl Into<Option<DeferredTraceFrame<'step>>>,
    ) -> Self {
        self.dep_error_frame = frame.into();
        self
    }

    /// Seal the envelope over a witnessed finish, applying the [`sealed_done`] projection here, at
    /// construction. `host` is the region of the frame the park keeps — the slot's own cart — and
    /// hosts both the erased closure and every list capture inside it.
    pub(in crate::machine::execute) fn finish_witnessed<F>(
        self,
        host: RegionBrand<'step>,
        finish: F,
    ) -> Outcome<'step>
    where
        F: for<'view, 'd> Fn(
                &DecideCtx<'_, 'step, 'view>,
                &[DepTerminal<'d>],
            ) -> Result<StepCarried<'step>, KError>
            + Copy
            + 'step,
    {
        self.finish_terminal(host, sealed_done(finish))
    }

    /// Seal the envelope over a terminal finish: compose the dep-error gate around it and erase
    /// once, onto the bumped tier in `host`. The gate's frame is baked in here *and* kept on the
    /// park, which the apply's install-and-inspect path reads to label an already-errored producer.
    pub(in crate::machine::execute) fn finish_terminal<F>(
        self,
        host: RegionBrand<'step>,
        finish: F,
    ) -> Outcome<'step>
    where
        F: for<'view, 'd> Fn(&DecideCtx<'_, 'step, 'view>, &[DepTerminal<'d>]) -> Outcome<'step>
            + Copy
            + 'step,
    {
        Outcome::Park {
            deps: self.deps,
            continuation: Continuation::Ready(erase_bumped(
                host,
                gated(self.dep_error_frame, finish),
            )),
            dep_error_frame: self.dep_error_frame,
        }
    }

    /// [`Self::finish_terminal`]'s owning twin, for the finishes that stay on the boxed tier: an
    /// awaiting builtin `Action`'s lowering, whose
    /// [`AwaitContinue::Boxed`](crate::machine::core::AwaitContinue) arm owns captures with drop
    /// glue.
    pub(in crate::machine::execute) fn finish_terminal_boxed<F>(self, finish: F) -> Outcome<'step>
    where
        F: for<'view, 'd> FnOnce(
                &DecideCtx<'_, 'step, 'view>,
                &[DepTerminal<'d>],
            ) -> Outcome<'step>
            + 'step,
    {
        Outcome::Park {
            deps: self.deps,
            continuation: Continuation::Ready(erase_boxed(gated_once(
                self.dep_error_frame,
                finish,
            ))),
            dep_error_frame: self.dep_error_frame,
        }
    }
}

/// The resolved dep terminal every finish reads — a delivered resident of a region this step
/// already covers, re-branded once at step start. Lives in core so the builtin-`Action` currency
/// can name it.
pub(in crate::machine::execute) use crate::machine::core::DepTerminal;

/// The one continuation every node runs when its deps resolve — the unified currency
/// [`NodeWork`](super::nodes::NodeWork) carries: the slot's [`ParkState`] as plain data beside a
/// two-tier `call` target. The step deposits that state into the ambient slot-step context and then
/// runs the call, so carrying a checker down a tail chain — or a block frame across a park — costs
/// a field rather than a wrapping closure.
///
/// Dep terminals reach the call in submission order, and an errored dep is *not* short-circuited
/// here: the closure decides. Per-family behaviour — the dep-error gate, the witnessed projection,
/// the catch lift — composes generically at the construction site ([`gated`], [`sealed_done`],
/// [`catching`], [`decide_only`]) and crosses into storage through a single erasure.
pub(in crate::machine::execute) struct NodeContinuation<'a> {
    pub(in crate::machine::execute) park: ParkState,
    pub(in crate::machine::execute) call: ContinuationCall<'a>,
}

impl<'a> NodeContinuation<'a> {
    /// The replacement door: a continuation that carries an obligation and no block frame — only a
    /// park keeps a frame, and a replacement rebuilds its placement from its own outcome.
    pub(in crate::machine::execute) fn new(
        obligation: Option<ReturnObligation>,
        call: ContinuationCall<'a>,
    ) -> Self {
        NodeContinuation {
            park: ParkState {
                obligation,
                block_frame: None,
            },
            call,
        }
    }

    /// The park door: the whole ambient state crosses the dormancy, so the woken step re-deposits
    /// what its parking step established.
    pub(in crate::machine::execute) fn parked(park: ParkState, call: ContinuationCall<'a>) -> Self {
        NodeContinuation { park, call }
    }
}

/// The stored call shape, as the re-entrant `Fn` the bumped tier holds: its closure is `Copy`, so
/// nothing about running it consumes it.
type BumpedCall<'a> = dyn for<'view, 'd> Fn(
        &DecideCtx<'_, 'a, 'view>,
        &[Result<DepTerminal<'d>, KError>],
        NodeId,
    ) -> Outcome<'a>
    + 'a;

/// The same call shape as the one-shot `FnOnce` the boxed tier holds, consumed by the step that
/// runs it so its owned captures drop with it.
type BoxedCall<'a> = dyn for<'view, 'd> FnOnce(
        &DecideCtx<'_, 'a, 'view>,
        &[Result<DepTerminal<'d>, KError>],
        NodeId,
    ) -> Outcome<'a>
    + 'a;

/// Where a stored continuation's closure lives — the two-tier erase door's product. Both tiers
/// cross the same seal; the tier decides only where the bytes sit and whether drop glue runs.
pub(in crate::machine::execute) enum ContinuationCall<'a> {
    /// A `Copy` closure bump-allocated into the region of the frame the work installs under, called
    /// by reference. `Copy` captures make the closure `Fn` and keep the host region Drop-free, the
    /// invariant [`BumpAllocator::value`]'s `T: Copy` guard enforces.
    Bumped(&'a BumpedCall<'a>),
    /// An owning closure on the heap; its captures' destructors run at slot death.
    Boxed(Box<BoxedCall<'a>>),
}

/// Erase a `Copy` closure onto the bumped tier, hosting its bytes in `host` — the region of the
/// frame the work installs under (the current frame for a park or an in-place replace).
///
/// The parameter is a [`RegionBrand`], not a bare [`BumpAllocator`], deliberately: the step scratch
/// arena carries no brand, so a scratch-hosted continuation — dangling at the next drain pop — is
/// unrepresentable through this door.
pub(in crate::machine::execute) fn erase_bumped<'step, F>(
    host: RegionBrand<'step>,
    f: F,
) -> ContinuationCall<'step>
where
    F: for<'view, 'd> Fn(
            &DecideCtx<'_, 'step, 'view>,
            &[Result<DepTerminal<'d>, KError>],
            NodeId,
        ) -> Outcome<'step>
        + Copy
        + 'step,
{
    ContinuationCall::Bumped(host.handle().allocator().value(f))
}

/// Erase an owning closure onto the boxed tier — the tier for a continuation whose captures need
/// drop glue (a `KError`, an `Rc` frame).
pub(in crate::machine::execute) fn erase_boxed<'step, F>(f: F) -> ContinuationCall<'step>
where
    F: for<'view, 'd> FnOnce(
            &DecideCtx<'_, 'step, 'view>,
            &[Result<DepTerminal<'d>, KError>],
            NodeId,
        ) -> Outcome<'step>
        + 'step,
{
    ContinuationCall::Boxed(Box::new(f))
}

/// `Reattachable` family for the [`NodeContinuation`]: its [`ContinuationCall::Boxed`] arm owns its
/// captures, so the family takes the `droppable` arm and rests as
/// `SealedPinned<ContinuationFamily, Rc<SlotFrame>>`, sealed against the slot's anchor at install.
/// That anchor is what covers the run-lived data the continuation reads — the parked AST, a finish
/// closure's captured scope, a bumped closure's host region — held in the run region or a strict
/// ancestor of the slot's per-call cart for the whole dormant life, which is the owned tier's
/// standing obligation ([workgraph/design/witnessed-memory.md § What a droppable family
/// accepts](../../../workgraph/design/witnessed-memory.md#what-a-droppable-family-accepts)).
pub(in crate::machine::execute) struct ContinuationFamily;

reattachable!(droppable ContinuationFamily => NodeContinuation<'r>);

/// Collect the step's resolved dep terminals, or short-circuit on the first errored one. The
/// terminals land on the step's scratch arena at their exactly-known length: `DepTerminal` is
/// `Copy` data, so the finish reads a contiguous slice of values with no per-dep indirection.
fn all_or_first_error<'s, 'd>(
    scratch: BumpAllocator<'s>,
    results: &[Result<DepTerminal<'d>, KError>],
    dep_error_frame: Option<DeferredTraceFrame<'_>>,
    registries: &RunRegistries,
) -> Result<BumpVec<'s, DepTerminal<'d>>, KError> {
    let mut terminals = BumpVec::with_capacity_in(results.len(), scratch);
    for r in results {
        match r {
            Ok(t) => terminals.push(*t),
            Err(e) => return Err(propagate_dep_error(e, dep_error_frame, registries)),
        }
    }
    Ok(terminals)
}

/// Adapt a dep-reading `finish` onto the uniform continuation signature behind the dep-error gate:
/// short-circuit on the first errored dep (labelled with `dep_error_frame`), else hand `finish` the
/// resolved terminals. The one delivery loop every dep-finish runs through.
pub(in crate::machine::execute) fn gated<'step, F>(
    dep_error_frame: Option<DeferredTraceFrame<'step>>,
    finish: F,
) -> impl for<'view, 'd> Fn(
    &DecideCtx<'_, 'step, 'view>,
    &[Result<DepTerminal<'d>, KError>],
    NodeId,
) -> Outcome<'step>
+ Copy
+ 'step
where
    F: for<'view, 'd> Fn(&DecideCtx<'_, 'step, 'view>, &[DepTerminal<'d>]) -> Outcome<'step>
        + Copy
        + 'step,
{
    move |view: &DecideCtx<'_, 'step, '_>,
          results: &[Result<DepTerminal<'_>, KError>],
          _id: NodeId| {
        let terminals =
            match all_or_first_error(view.scratch(), results, dep_error_frame, view.registries()) {
                Ok(terminals) => terminals,
                Err(e) => return Outcome::Done(Err(e)),
            };
        finish(view, &terminals)
    }
}

/// [`gated`]'s one-shot twin for the boxed tier: same delivery loop over a finish that consumes its
/// owned captures.
pub(in crate::machine::execute) fn gated_once<'step, F>(
    dep_error_frame: Option<DeferredTraceFrame<'step>>,
    finish: F,
) -> impl for<'view, 'd> FnOnce(
    &DecideCtx<'_, 'step, 'view>,
    &[Result<DepTerminal<'d>, KError>],
    NodeId,
) -> Outcome<'step>
+ 'step
where
    F: for<'view, 'd> FnOnce(&DecideCtx<'_, 'step, 'view>, &[DepTerminal<'d>]) -> Outcome<'step>
        + 'step,
{
    move |view: &DecideCtx<'_, 'step, '_>,
          results: &[Result<DepTerminal<'_>, KError>],
          _id: NodeId| {
        let terminals =
            match all_or_first_error(view.scratch(), results, dep_error_frame, view.registries()) {
                Ok(terminals) => terminals,
                Err(e) => return Outcome::Done(Err(e)),
            };
        finish(view, &terminals)
    }
}

/// Project a witnessed finish — one folding the resolved dep terminals into the aggregate's
/// witnessed carrier, so the result names every region it reaches by construction — onto the
/// terminal-finish delivery [`gated`] runs. Its `Result` is the shape channel: a shape error (a
/// non-scalar dict key) short-circuits to [`Outcome::Done`].
pub(in crate::machine::execute) fn sealed_done<'step, F>(
    finish: F,
) -> impl for<'view, 'd> Fn(&DecideCtx<'_, 'step, 'view>, &[DepTerminal<'d>]) -> Outcome<'step>
+ Copy
+ 'step
where
    F: for<'view, 'd> Fn(
            &DecideCtx<'_, 'step, 'view>,
            &[DepTerminal<'d>],
        ) -> Result<StepCarried<'step>, KError>
        + Copy
        + 'step,
{
    move |view: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| match finish(
        view, terminals,
    ) {
        Ok(carrier) => Outcome::Done(Ok(carrier)),
        Err(e) => Outcome::Done(Err(e)),
    }
}

/// Adapt a catch finish onto the uniform signature: no short-circuit on an errored dep, so the
/// closure can recover or re-raise. The watched producer's delivered resident is lifted back into
/// an envelope owning its whole reach, which the finish adopts or opens at its own step brand.
/// `Copy` captures in and out, so the product erases on the bumped tier.
pub(in crate::machine::execute) fn catching<'step, F>(
    finish: F,
) -> impl for<'view, 'd> Fn(
    &DecideCtx<'_, 'step, 'view>,
    &[Result<DepTerminal<'d>, KError>],
    NodeId,
) -> Outcome<'step>
+ Copy
+ 'step
where
    F: for<'view> Fn(
            &DecideCtx<'_, 'step, 'view>,
            Result<DeliveredCarried, KError>,
        ) -> Outcome<'step>
        + Copy
        + 'step,
{
    move |view: &DecideCtx<'_, 'step, '_>,
          results: &[Result<DepTerminal<'_>, KError>],
          _id: NodeId| {
        let result = match &results[0] {
            Ok(t) => Ok(view.current_scope().lift_spliced(&t.cell)),
            // Frameless: the recovery-site dispatch attaches its own frame.
            Err(e) => Err(propagate_dep_error(e, None, view.registries())),
        };
        finish(view, result)
    }
}

/// Adapt a dispatch decide onto the uniform signature: it takes no dep values (it reads the view
/// and spawns / re-resolves), so the results slice is dropped. `Copy` captures in and out, so the
/// product erases on the bumped tier.
pub(in crate::machine::execute) fn decide_only<'step, F>(
    resume: F,
) -> impl for<'view, 'd> Fn(
    &DecideCtx<'_, 'step, 'view>,
    &[Result<DepTerminal<'d>, KError>],
    NodeId,
) -> Outcome<'step>
+ Copy
+ 'step
where
    F: for<'view> Fn(&DecideCtx<'_, 'step, 'view>, NodeId) -> Outcome<'step> + Copy + 'step,
{
    move |view: &DecideCtx<'_, 'step, '_>,
          _results: &[Result<DepTerminal<'_>, KError>],
          id: NodeId| resume(view, id)
}

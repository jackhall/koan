//! The write harness. [`KoanRuntime`] owns the [`Scheduler`] by composition and is the sole holder
//! of `&mut Scheduler` across the execute tree — the AST-aware submission wrappers, the execute
//! loop, and [`KoanRuntime::apply_outcome`] (the one graph writer) hang off it. Its read surface
//! forwards to the owned scheduler.
//!
//! [`run_action`] is the shared *action* harness: a pure `Action -> Outcome` decide that reads a
//! [`SchedulerView`] and issues no graph write. Both `KFunction::invoke` (lowering an
//! `ExecOutcome → Action`) and every `Action`-authored builtin route through it. The peer of
//! `dispatch/exec.rs::invoke`. The `Action` *types* live in
//! [`crate::machine::core::kfunction::action`].
//!
//! The [`interpret`] submodule holds the program entry points ([`interpret`], [`interpret_with_writer`],
//! [`interpret_with_writer_path`]); they parse, stand up the region/root scope, and drive the run via
//! [`KoanRuntime::run_program`]. The [`submit`] submodule holds the AST-aware dispatch-submission
//! wrappers ([`KoanRuntime::enter_block`], [`KoanRuntime::dispatch_in_scope`], `dispatch_in_own_scope`,
//! `dispatch_body`, `submit_dep_finish_in_own_scope`) — the only callers that turn a
//! [`WorkingExpression`] into scheduler work.

use std::rc::Rc;

use crate::machine::core::ReturnContract;
use crate::machine::core::{
    Action, ActionKind, BlockEntry, DepPlacement, FinishCtx, FramePlacement, TailContract,
};
use crate::machine::core::{ProgramBrand, RegionBrand, ScopeRefFamily};
use crate::machine::model::Carried;
use crate::machine::model::{ExpressionPart, Part, PartClass, WorkingExpression, WorkingPart};
use crate::machine::{
    CallFrame, CarrierWitness, DeliveredCarried, FrameStorage, KError, KErrorKind, NodeId,
};
use crate::witnessed::SealedExtern;

use super::dispatch::{
    BodyPlacement, DepRequest, SchedulerView, SubmitContext, propagate_dep_error,
};
use super::finalize::check_spliced_return;
use super::lift::relocate_seam;
use super::nodes::{ChainOp, NodeStep, NodeWork};
use super::obligation::{ReturnObligation, with_obligation};
use super::outcome::{Await, Continuation, Outcome, TerminalDepFinish, dep_error_frame};
use super::run_loop::DestHandleFamily;
use super::{
    CatchFinish, ContinuationFamily, catch_continuation, ignore_results, seal_witnessed,
    short_circuit,
};
use crate::machine::model::CarriedFamily;
use crate::scheduler::{Deps, EdgeId, InstalledEdge, Scheduler, Workload};
use crate::witnessed::Delivered;

mod interpret;
mod submit;

pub(crate) use interpret::seed_run_root;
pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};

/// The Koan instantiation of the scheduler's [`Workload`] interface — the marker that binds the
/// opaque scheduler types to their concrete Koan forms. The scheduler is generic over `W: Workload`
/// and names none of these directly; the workload side (this module, `dispatch/**`) supplies them.
pub(in crate::machine::execute) struct KoanWorkload;

impl Workload for KoanWorkload {
    type Value = CarriedFamily;
    type Error = KError;
    type Profile = crate::machine::core::KoanStorageProfile;
    type Frame = super::nodes::SlotFrame;
    type Continuation = ContinuationFamily;

    /// Koan's half of delivery: the value-level escape seam ([`relocate_seam`]) — the cost-driven
    /// copy-or-pin verdict with the retention claim derived from the rebuilt product. The walk
    /// decides which destinations a terminal lands in; this decides what the crossing costs.
    fn deliver(
        terminal: &DeliveredCarried,
        dest: Delivered<DestHandleFamily, CarrierWitness, FrameStorage>,
    ) -> DeliveredCarried {
        relocate_seam(terminal, dest)
    }
}

/// The write harness: the sole holder of `&mut Scheduler` across the execute tree. It owns the
/// [`Scheduler`] by composition (a `sched` field, not a `&mut` borrow) and carries every AST-aware
/// and graph-mutating step — the execute loop, [`Self::apply_outcome`], the dispatch-submission
/// wrappers, `submit_expression`, and the literal lowering. A dispatch *decide* runs against a
/// read-only [`SchedulerView`](super::dispatch::SchedulerView) over `&self.sched` and returns an
/// [`Outcome`]; only the harness reborrows the scheduler mutably to apply it. So "everything outside
/// the harness is read-only" is structurally enforced, not a naming convention.
///
/// See design/execution/README.md § the dispatcher / scheduler boundary.
pub struct KoanRuntime<'run> {
    pub(in crate::machine::execute) sched: Scheduler<KoanWorkload>,
    /// The ambient per-step context — the active per-call frame, run frame, the
    /// executing slot's payload, and the contract-chain flag. The scheduler is a pure DAG runtime;
    /// this driver-side state floats across a single step. See [`ambient`](super::ambient).
    pub(in crate::machine::execute) ambient: super::ambient::AmbientContext,
    /// This run's program storage capability, handed to every step through its
    /// [`SchedulerView`](super::dispatch::SchedulerView). It also carries `'run`: the scheduler is
    /// value-erased (`Scheduler<KoanWorkload>`), so without this the run lifetime would live only in
    /// the harness's own method signatures.
    pub(in crate::machine::execute) program: ProgramBrand<'run>,
    /// The run's output sink, held only until [`ensure_run_frame`](Self::ensure_run_frame) mints
    /// the run frame and moves it onto that frame — the writer's real home, beside the run's
    /// [`TypeRegistry`](crate::machine::model::TypeRegistry). It waits here rather than being
    /// passed at the mint because the mint is lazy: a dispatch into the run scope establishes the
    /// frame if nothing has yet, and that call site has no writer to supply.
    pub(in crate::machine::execute) writer: Option<Box<dyn std::io::Write>>,
}

impl<'run> KoanRuntime<'run> {
    /// `out` is where this run's `PRINT` output goes; it lands on the run frame the first
    /// submission establishes.
    pub fn new(program: ProgramBrand<'run>, out: Box<dyn std::io::Write>) -> Self {
        Self {
            sched: Scheduler::new(),
            ambient: super::ambient::AmbientContext::default(),
            program,
            writer: Some(out),
        }
    }

    /// Drop the scheduler's slot store and start a fresh one, keeping the ambient run frame — and
    /// with it the run's [`TypeRegistry`](crate::machine::model::TypeRegistry) and every binding
    /// already installed on the run root. Call at quiescence.
    ///
    /// This is the teardown a test needs between phases when it measures something the drained
    /// slots hold onto: the scheduler's slot store is a free-list whose length is a high-water
    /// mark, and a finished slot's terminal retains its producer frame. Both are program-lifetime
    /// facts about the scheduler, not the run, so a test measuring one program's slot footprint or
    /// frame retention releases the prior phase's slots first.
    #[cfg(test)]
    pub(crate) fn reset_slots(&mut self) {
        self.sched = Scheduler::new();
    }
}

/// Read forwarders to the owned [`Scheduler`]. The harness exposes the scheduler's read surface
/// (terminal reads / slot count) so callers drive the whole run through the harness without ever
/// borrowing the scheduler — the write methods are the inherent `&mut self` ones above.
impl<'run> KoanRuntime<'run> {
    /// Open an edge's delivered terminal at a rank-2 brand and hand the value to `f`, returning its
    /// result or the terminal's error — the destination-verb read. See
    /// [`Scheduler::read_edge_result_with`].
    pub fn read_edge_result_with<R>(
        &self,
        edge: EdgeId,
        f: impl for<'b> FnOnce(crate::machine::model::Carried<'b>) -> R,
    ) -> Result<R, &KError> {
        self.sched.read_edge_result_with(edge, f)
    }

    /// An edge's delivered terminal's error, or `Ok(())` on success — the value-free probe.
    /// See [`Scheduler::edge_result_error`].
    pub fn edge_result_error(&self, edge: EdgeId) -> Result<(), &KError> {
        self.sched.edge_result_error(edge)
    }

    /// Re-brand the edge's delivered resident against `scope`'s own region owner and lift it back
    /// into an envelope — the same two moves the run loop makes at step start, for a test harness
    /// that goes on to copy the value out. The edge is destined at `scope`'s region, so holding that
    /// owner across the brand is exactly what makes the lift's upgrade succeed.
    #[cfg(test)]
    pub(crate) fn edge_delivered(
        &self,
        edge: EdgeId,
        scope: &crate::machine::Scope<'_>,
    ) -> Result<DeliveredCarried, KError> {
        let resident = self.sched.edge_resident(edge)?;
        let coverage = crate::machine::FrameCoverage::of(
            scope
                .region_owner()
                .upgrade()
                .expect("a live scope reference implies a live region owner"),
        );
        Ok(scope.lift_spliced(&resident.brand_with(&coverage)))
    }

    /// Wire an edge onto `producer`, destined at `scope`'s own region — the test harness's
    /// stand-in for the root edge `run_program` installs, and for the placeholder edge a real
    /// binder plan claims. Slots reclaim at finalize, so a test that reads a result holds an edge
    /// exactly as production does; wiring it here, right after the dispatch that allocated the
    /// slot, is the same pre-terminal wiring both production callers do.
    pub(crate) fn install_edge_for_test(
        &mut self,
        producer: NodeId,
        scope: &crate::machine::Scope<'_>,
    ) -> EdgeId {
        let destination = scope
            .region_owner()
            .upgrade()
            .expect("a live scope reference implies a live region owner");
        self.sched.install_edge(producer, &destination)
    }

    pub fn len(&self) -> usize {
        self.sched.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sched.is_empty()
    }
}

/// Test-only forwarders: an immutable `&Scheduler` view (`resolve_name_part` fixtures) plus a
/// slot's stored chain. No `&mut Scheduler` escapes — the accessor hands out `&Scheduler`, keeping
/// the harness the sole writer.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    pub(in crate::machine::execute) fn scheduler(&self) -> &Scheduler<KoanWorkload> {
        &self.sched
    }

    pub fn chain_of(&self, id: NodeId) -> Option<Rc<crate::machine::LexicalFrame>> {
        self.sched.anchor_of(id).map(|a| a.payload.chain.clone())
    }
}

/// Lower an [`Action`] into the scheduler's [`Outcome`] currency — an `Action -> Outcome` transform
/// that issues no graph write: a `AwaitDeps`/`Catch` declares its deps (and a wrapped finish that
/// recurses `run_action` on the `AwaitContinue`/`CatchContinue` it produces) as a [`Outcome::ParkThenContinue`],
/// and the harness submits and applies. Every scheduler read the body needs is deferred into the
/// finish, which sees a read-only [`SchedulerView`](super::dispatch::SchedulerView) at wake.
///
/// `view` is the executing step's read view: a tail `Action` reads its established
/// declared-return obligation off it (the ambient slot-step state) to decide keep-first and wrap the
/// replacement continuation. A finish that emits its `Continue` later reads its own wake-time view
/// instead, so the obligation it sees is the one its park deposit re-installed.
/// The block scope id a [`BlockEntry`] names — the input the chain reshape ([`ChainOp::decide`])
/// reads alongside the contract variant. `None` for a blockless (frameless) tail.
fn block_entry_scope(block_entry: &BlockEntry<'_>) -> Option<crate::machine::core::ScopeId> {
    match block_entry {
        BlockEntry::None => None,
        BlockEntry::FrameScope(frame) => Some(frame.scope_id()),
        BlockEntry::Overlay(overlay) => Some(overlay.id),
    }
}

/// Split a body block into the statements it fans out to, one working node apiece. The
/// scheduler-side peer of [`split_body_statements`](crate::machine::split_body_statements): a block
/// (two or more parts, every one an expression) yields its children, and any other body is the one
/// statement it already is. A parsed child crosses into the scheduler here, at `brand`; a child the
/// scheduler synthesized is already working form.
fn split_working_body<'a>(
    brand: RegionBrand<'a>,
    body: WorkingExpression<'a>,
) -> Vec<WorkingExpression<'a>> {
    let is_block = body.parts.len() >= 2
        && body
            .parts
            .iter()
            .all(|part| matches!(part.value.class(), PartClass::Expression));
    if !is_block {
        return vec![body];
    }
    body.parts
        .iter()
        .filter_map(|part| match part.value {
            WorkingPart::Ast(ExpressionPart::Expression(child)) => {
                Some(WorkingExpression::from_ast(brand, *child))
            }
            WorkingPart::Expression(child) => Some(*child),
            _ => None,
        })
        .collect()
}

pub(in crate::machine::execute) fn run_action<'step>(
    view: &SchedulerView<'_, 'step, '_>,
    action: Action<'step>,
) -> Outcome<'step> {
    // The step's binding-table writes travel as outcome data: deposit them into the run-loop-owned
    // sink in the order the bodies decided them, before interpreting what happens next. Every
    // recursive arm below (a wake-time finish's `Action`) deposits through this same call, so a
    // chain of finishes contributes its writes in program order.
    view.deposit_effects(action.effects);
    match action.next {
        // Already a step-branded carrier (or error): `finalize` seals it as-is, no co-location
        // bundle.
        ActionKind::Done(result) => Outcome::Done(result),

        ActionKind::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        } => {
            // A block-entering tail sits above the params (`1`) or the leading siblings (`N`); a
            // frameless continuation keeps the slot's block at index `0`.
            let body_index = if matches!(block_entry, BlockEntry::None) {
                0
            } else {
                leading.len() + 1
            };
            if leading.is_empty() {
                // No leading statements: tail-replace directly into the tail body.
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    TailContract::FromLastResult { .. } => {
                        unreachable!(
                            "a from-last-result contract rides at least its type statement"
                        )
                    }
                };
                // Decide the chain reshape from this call's still-live contract variant, then
                // keep-first the obligation: the chain's established obligation (deposited on the
                // step view) wins over this call's own contract, which is sealed only when no chain
                // is yet established. The winner is wrapped onto the replacement continuation so the
                // next step re-deposits it (see [`with_obligation`](super::obligation)).
                let chain = ChainOp::decide(
                    block_entry_scope(&block_entry),
                    contract.as_ref(),
                    body_index,
                );
                let winner = view
                    .current_obligation_duplicate()
                    .or_else(|| contract.map(ReturnObligation::seal));
                return Outcome::Continue {
                    work: super::dispatch::decide_tail(tail, winner),
                    frame: frame_placement,
                    chain,
                    block_entry,
                };
            }
            // Leading statements become owned siblings in the block (one `BodyBlock` dep); the slot
            // parks on them so they run — and cascade-free — before the tail continues. Where they
            // bind is what `block_entry` names: the block frame's own scope (MATCH / TRY arms via a
            // pre-built `FreshChild` cart, FN-body tails re-entering the already-installed cart with
            // `Inherit`), or a caller-allocated overlay under the inherited call-site cart (USING).
            let placement = match &block_entry {
                BlockEntry::FrameScope(frame) => BodyPlacement::Frame(Rc::clone(frame)),
                BlockEntry::Overlay(overlay) => BodyPlacement::Overlay(overlay),
                BlockEntry::None => unreachable!("a leading-carrying tail enters a block"),
            };
            // `FreshTail` mints its cart only at apply time — after the leading statements would
            // already have fanned out — so a leading-carrying tail cannot ride it.
            debug_assert!(
                !matches!(frame_placement, FramePlacement::FreshTail { .. }),
                "a leading-carrying tail is a FreshChild frame, an Inherit cart, or an overlay"
            );
            let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    // The return-type expression is the last leading statement (all owned), so its
                    // resolved value is the last owned terminal, read in place in the region it was
                    // delivered into. The per-call type is re-homed into the captured-scope region —
                    // a strict ancestor the cart keeps live — like the `Type` form's `PerCall.ret`.
                    TailContract::FromLastResult { func } => {
                        let owned = terminals.owned_slice();
                        let terminal = owned[owned.len() - 1];
                        let opened = terminal.cell.open_at();
                        let kt = match opened.value() {
                            Carried::Type(t) => t,
                            Carried::Object(other) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                                    format!(
                                        "FN deferred return-type expression produced a non-type {} value",
                                        other.ktype().name(view.types()),
                                    ),
                                ))));
                            }
                            Carried::UnresolvedType(ti) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
                                    ti.render(),
                                ))));
                            }
                        };
                        // The resolved type is a `Copy` handle, so the contract carries it directly
                        // and outlives the sub-dispatch's terminal without naming any region.
                        Some(ReturnContract::PerCall { func, ret: kt })
                    }
                };
                // Decide the chain reshape and keep-first the obligation as on the leading-free
                // path, but against this finish's own wake-time view: the park that carried the
                // leading statements re-deposited the established obligation, so a chain checks its
                // first caller's declared return rather than this resolving tail's.
                let chain = ChainOp::decide(
                    block_entry_scope(&block_entry),
                    contract.as_ref(),
                    body_index,
                );
                let winner = view
                    .current_obligation_duplicate()
                    .or_else(|| contract.map(ReturnObligation::seal));
                Outcome::Continue {
                    work: super::dispatch::decide_tail(tail, winner),
                    frame: frame_placement,
                    chain,
                    block_entry,
                }
            });
            Await::on(Deps::from_owned([DepRequest::BodyBlock {
                statements: leading,
                placement,
            }]))
            .error_frame(dep_error_frame())
            .finish_terminal(finish)
        }

        ActionKind::AwaitDeps { deps, finish } => {
            // The builtin assembled the structural `[park..., owned...]` split itself: parks keep
            // first-occurrence order, owned insertion order, and the builder delivers results
            // `[park..., owned...]`. This arm maps each owned sub-dispatch into the library dep
            // currency and rebuilds the `Deps` envelope `Await::on` consumes; the wrapped finish
            // recurses `run_action` on the `AwaitContinue`.
            let (parks, owned) = deps.into_parts();
            let mut lowered: Deps<DepRequest<'step>> = Deps::from_parks(parks);
            for sub in owned {
                lowered.own(sub.into_request());
            }
            let wrapped: TerminalDepFinish<'step> = Box::new(move |view, results| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    ctx: view.step_ctx(),
                    types: view.types(),
                };
                run_action(view, finish(&fctx, results))
            });
            Await::on(lowered)
                .error_frame(dep_error_frame())
                .finish_terminal(wrapped)
        }

        ActionKind::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a dep-finish body's fan-out.
            let wrapped: CatchFinish<'step> = Box::new(move |view, result| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    ctx: view.step_ctx(),
                    types: view.types(),
                };
                run_action(view, finish(&fctx, result))
            });
            Outcome::ParkThenContinue {
                deps: Deps::new(),
                continuation: Continuation::Catch {
                    watched,
                    finish: wrapped,
                },
                dep_error_frame: None,
            }
        }
    }
}

/// The write-harness apply path — the one place that turns a decided [`Outcome`] into the scheduler
/// graph writes it implies and the terminal [`NodeStep`]. A shape handler decides against a
/// read-only [`SchedulerView`](super::dispatch::SchedulerView) and returns an outcome; this applies
/// it. `KoanRuntime` holds the sole `&mut Scheduler`, so this is the only path that mutates the
/// graph in response to a dispatch decide.
impl<'run> KoanRuntime<'run> {
    /// Realize a single-statement dispatch dep at `placement` to its producer slot. `OwnScope`
    /// re-dispatches against the executing slot's own scope; `InScope` enters a fresh
    /// **single-statement** block (so an inner `LET` stays local). A multi-statement body splits
    /// separately — see the `InScope` arm of [`Self::apply_outcome`] and [`Self::dispatch_body`].
    fn realize_dispatch<'a>(
        &mut self,
        expr: WorkingExpression<'a>,
        placement: DepPlacement<'a>,
    ) -> NodeId {
        match placement {
            DepPlacement::OwnScope => self.dispatch_in_own_scope(expr, SubmitContext::SubDispatch),
            DepPlacement::InScope(scope) => self
                .enter_block(scope.id, vec![expr], scope)
                .into_iter()
                .next()
                .expect("enter_block of one statement yields one node"),
        }
    }

    /// Realize one staged eager dep as its producer node — the four shapes
    /// [`stage_eager_part`](super::dispatch::stage_eager_part) emits. `brand` is the realizing
    /// step's, where an aggregate literal's per-element dispatch node is bumped. `BodyBlock` never
    /// reaches here (the stager doesn't produce it).
    pub(in crate::machine::execute) fn realize_eager_dep<'a>(
        &mut self,
        brand: RegionBrand<'a>,
        dep: DepRequest<'a>,
    ) -> NodeId {
        match dep {
            DepRequest::Dispatch { expr, placement } => self.realize_dispatch(expr, placement),
            DepRequest::ListLit(items) => self.schedule_list_literal(brand, items),
            DepRequest::DictLit(pairs) => self.schedule_dict_literal(brand, pairs),
            DepRequest::RecordLit(fields) => self.schedule_record_literal(brand, fields),
            DepRequest::BodyBlock { .. } => {
                unreachable!("eager staging emits only Dispatch / literal deps")
            }
        }
    }

    /// Realize a [`Catch`](Continuation::Catch)'s single watched [`DepRequest`] to a producer
    /// `NodeId`: a `Dispatch` realizes as a single statement (an `InScope` watched expr enters a
    /// fresh single-statement block — see [`Self::realize_dispatch`]). A `Catch` never watches a
    /// dispatcher-only lowering.
    fn realize_catch_dep<'a>(&mut self, dep: DepRequest<'a>) -> NodeId {
        match dep {
            DepRequest::Dispatch { expr, placement } => self.realize_dispatch(expr, placement),
            DepRequest::ListLit(_)
            | DepRequest::DictLit(_)
            | DepRequest::RecordLit(_)
            | DepRequest::BodyBlock { .. } => {
                unreachable!("a Catch watches only a simple Dispatch dep")
            }
        }
    }

    /// Resolve a [`FramePlacement`] to the cart a [`Continue`](Outcome::Continue) installs: mint a
    /// fresh TCO tail-call cart, take a builtin-minted fresh cart, or keep the current cart
    /// (`None`). The one place the placement → cart mapping lives — shared by the `Continue` body
    /// re-run and the folded invoke / re-resolve paths (which reach it through their own
    /// `Continue`).
    fn resolve_frame_placement<'x>(
        &mut self,
        placement: FramePlacement<'x>,
    ) -> Option<Rc<CallFrame>> {
        match placement {
            FramePlacement::FreshTail { outer } => Some(CallFrame::new(outer)),
            FramePlacement::FreshChild { frame } => Some(frame),
            FramePlacement::Inherit => None,
        }
    }

    /// Close the active frame's scope iff this slot owns it: the per-call frame's body has finished
    /// (a `Done` return, or a tail `Continue` retiring this iteration), so the scope
    /// takes no further binds and its reach-set seals. A `Yoked` sub-expression slot owns no frame
    /// (its `owner` never names this slot), so its `Done` is a no-op here.
    fn close_owned_scope(&self, id: NodeId) {
        if let Some(frame) = self.ambient.active_frame_ref()
            && frame.owner() == Some(id)
        {
            frame.with_scope(|s| s.close());
        }
    }

    /// Interpret an [`Outcome`] into the scheduler effect it names and return the slot's
    /// [`NodeStep`]. This is the sole graph writer the dispatch side reaches — a decide handler
    /// never holds `&mut Scheduler`.
    pub(in crate::machine::execute) fn apply_outcome<'step>(
        &mut self,
        outcome: Outcome<'step>,
        brand: RegionBrand<'step>,
        id: NodeId,
        anchor: &super::nodes::SlotFrame,
    ) -> NodeStep<'step> {
        match outcome {
            Outcome::Done(result) => {
                self.close_owned_scope(id);
                match result {
                    Ok(carrier) => NodeStep::DoneWitnessed(carrier),
                    Err(error) => NodeStep::Error(error),
                }
            }
            Outcome::Continue {
                work,
                frame,
                chain,
                block_entry,
            } => {
                // The body's leading statements are never dispatched here — a producer with leading
                // statements parks on them as owned `BodyBlock` deps and emits this `Continue` only
                // from the resolving finish (see `dispatch/exec.rs` and `run_action`).
                // A tail iteration (`FreshTail`) retires this scope before the fresh cart is
                // installed for the next; other placements keep the current scope live.
                if matches!(frame, FramePlacement::FreshTail { .. }) {
                    self.close_owned_scope(id);
                }
                let frame = self.resolve_frame_placement(frame);
                // The body re-dispatched into a freshly installed frame finalizes that frame's scope.
                if let Some(installed) = frame.as_ref() {
                    installed.set_owner(id);
                }
                // The chain reshape was decided at the `Continue` construction site while the
                // contract variant was live (see [`ChainOp`]); the run loop assembles it against the
                // post-step frame. An `Overlay` block entry also rides the tail slot's scope: erased
                // to a cart-witnessed carrier here (where the overlay is still live) so the frameless
                // `Replace` installs it as the slot's `YokedChild` — the frameless analogue of the
                // `Yoked` a framed tail re-projects from its own cart.
                let overlay_scope = match block_entry {
                    BlockEntry::Overlay(overlay) => {
                        Some(SealedExtern::<ScopeRefFamily>::erase(overlay))
                    }
                    BlockEntry::None | BlockEntry::FrameScope(_) => None,
                };
                NodeStep::Replace {
                    work,
                    frame,
                    chain,
                    overlay_scope,
                }
            }
            Outcome::ParkThenContinue {
                deps,
                continuation,
                dep_error_frame,
            } => {
                // Realize the builder's owned requests into producer ids; the park *sources* pass
                // through untouched for the door below to resolve. An `InScope`-placed `Dispatch`
                // and a `BodyBlock` each fan out to one owned producer per statement (so those arms
                // extend, the rest push one). Parks keep their first-occurrence order, owned their
                // realization order — the `[park..., owned...]` delivery order a finish addresses
                // through [`DepResults`].
                let (parks, owned_requests) = deps.into_parts();
                let mut owned: Vec<NodeId> = Vec::new();
                for dep in owned_requests {
                    match dep {
                        // An `InScope` body fans out one producer per statement (multi-statement
                        // split); `OwnScope` realizes as a single producer via the shared
                        // [`Self::realize_dispatch`].
                        DepRequest::Dispatch {
                            expr,
                            placement: DepPlacement::InScope(scope),
                            ..
                        } => {
                            let statements = split_working_body(scope.brand(), expr);
                            owned.extend(self.enter_block(scope.id, statements, scope));
                        }
                        dep @ (DepRequest::Dispatch { .. }
                        | DepRequest::ListLit(_)
                        | DepRequest::DictLit(_)
                        | DepRequest::RecordLit(_)) => {
                            let id = self.realize_eager_dep(brand, dep);
                            owned.push(id);
                        }
                        // A body block fans out one owned producer per statement: into a fresh
                        // per-call frame's own scope (`dispatch_body`), or — under `Inherit` — into a
                        // caller-allocated overlay via the same `enter_block` fan-out the leading
                        // statements of an `InScope` body use (USING).
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Frame(frame),
                        } => {
                            owned.extend(self.dispatch_body(&frame, statements));
                        }
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Overlay(overlay),
                        } => {
                            owned.extend(self.enter_block(overlay.id, statements, overlay));
                        }
                    }
                }
                // Wire the whole dep list through the one door: it mints this slot's own edge per
                // park source (inheriting the source's destination, so a park on a placeholder
                // delivers into the scope that placeholder named), installs each owned dep's
                // `Owned` edge (cascade-freed on resolve), and hands back the realized list plus
                // each park's filled-or-parked verdict. (`Catch` declares no deps here — it
                // realizes and owns its single watched dep in the `cont` match below.)
                let (resolved, installed) = self.sched.install_deps(id, &parks, &owned);
                // **Install-and-inspect**: a decide never probes a producer's standing, so a park
                // whose producer had already finalized is classified here. An errored one is
                // propagated now rather than waited on — a terminal slot never notifies again, so
                // the park would never wake. The first error wins, matching the step-start pull's
                // short-circuit. Rows and holds already installed discharge through this slot's
                // ordinary death path.
                for verdict in &installed {
                    let InstalledEdge::Filled(edge) = verdict else {
                        continue;
                    };
                    if let Err(dep_error) = self.sched.edge_result_error(*edge) {
                        let error = propagate_dep_error(dep_error, dep_error_frame.clone());
                        return self.apply_outcome(Outcome::Done(Err(error)), brand, id, anchor);
                    }
                }
                // Lower each variant to its outermost live `NodeContinuation` alongside the deps it
                // waits on and its deadlock-summary carrier, then wrap once below before erasing.
                let (deps, continuation, carrier) = match continuation {
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal dep-finish carries the `dep_error_frame()`
                    // label. Both install the same `Wait` over the realized deps (edges already
                    // installed above), the short-circuit baked into the continuation by
                    // `short_circuit` — the one loop the terminal delivery runs through. A finish whose
                    // value must outlive the resolving step folds the dep's carrier (`transfer_into`).
                    Continuation::FinishTerminal(finish) => {
                        (resolved, short_circuit(dep_error_frame, finish), None)
                    }
                    // The construction-inversion sibling: same realized deps and edges, but the
                    // `seal_witnessed` projection folds the resolved terminals (value + reach) into
                    // one witnessed carrier and seals as `Done(Ok)`.
                    Continuation::FinishWitnessed(finish) => (
                        resolved,
                        short_circuit(dep_error_frame, seal_witnessed(finish)),
                        None,
                    ),
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a dep-finish body's fan-out); realize and own it here.
                    // `catch_continuation` runs the finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let from = self.realize_catch_dep(watched);
                        let (watched_deps, _) = self.sched.install_deps(id, &[], &[from]);
                        (watched_deps, catch_continuation(finish), None)
                    }
                    // The resume closure carries the evolving `working_expr` from here on; the
                    // `carrier` it travels with is only the deadlock-summary sample. A decide takes
                    // no dep values, so `ignore_results` drops the (park-only) results view.
                    Continuation::Resume { carrier, resume } => {
                        (resolved, ignore_results(resume), carrier)
                    }
                };
                // Carry the ambient obligation across the park: the resumed step re-deposits it so
                // the chain's declared-return check still fires. The wrap sits on the outermost
                // closure, so every variant — including the dep-error short-circuit inside
                // `short_circuit` — runs under it and its Error arm still gets the trace label.
                let continuation = match self.ambient.current_obligation_duplicate() {
                    Some(obligation) => with_obligation(obligation, continuation),
                    None => continuation,
                };
                let work = NodeWork::new(deps, continuation, carrier);
                NodeStep::Replace {
                    work,
                    frame: None,
                    chain: ChainOp::Unchanged,
                    overlay_scope: None,
                }
            }
            Outcome::Forward(source) => {
                // The slot's result *is* the result behind `source`. Classification is the install's,
                // not a probe's: wiring a second edge off `source` answers filled-or-parked and
                // leaves a name this slot can read through. Filled: the producer already delivered
                // into the destination `source` names, and the new edge inherits that destination,
                // so the terminal is resident where this slot reads it and nothing relocates.
                // Parked: the probe edge has said all it can, so release it and `Alias` drives the
                // splice — move consumers onto the producer and alias the slot.
                // A forward is the one shape that wants no destination of its own: the slot is
                // standing in for the producer, so landing the terminal where the producer's own
                // consumers already look is the correct answer rather than a limitation. This is why
                // `install_edge_from`'s share-only filled branch covers every koan call site — no
                // site here asks for a delivery aimed at a region `source` does not already name.
                // The classification edge joins the slot's owned list: the run loop releases it when
                // the slot terminalizes (or splices out), which is what lets a checker micro-step
                // re-emit `Forward` on an edge of its own rather than on a foreign claim its binder
                // may have retired in the meantime.
                let installed = self.sched.install_edge_from(source);
                anchor.own_edges([installed.edge_id()]);
                let Some(obligation) = self.ambient.current_obligation_duplicate() else {
                    return match installed {
                        InstalledEdge::Filled(edge) => NodeStep::ForwardReady(edge),
                        InstalledEdge::Parked(_) => NodeStep::Alias(source),
                    };
                };
                // A residual declared-return obligation on this splice must be discharged before the
                // rehomed terminal reaches any consumer. Take it out of the ambient so neither this
                // step's finalize (the obligation is spent here) nor the not-ready micro-step's
                // continuation re-observes it; `obligation` is captured (never re-deposited), so the
                // check runs obligation-free.
                self.ambient.take_obligation();
                match installed {
                    // The producer resolved: run the declared-return check inline against its
                    // terminal, then behave as the obligation-free ready path. An errored producer
                    // carries no value to check — `ForwardReady` relocates its error as the
                    // obligation-free path would.
                    InstalledEdge::Filled(edge) => {
                        // The producer's value is already resident in the edge's destination; the
                        // check reads it in place, under the region's own owner.
                        let checked = match self.sched.read_edge_result_with(edge, |value| {
                            check_spliced_return(&obligation, value, self.ambient.type_registry())
                        }) {
                            Ok(checked) => checked,
                            // A ready-but-errored producer carries no value to check.
                            Err(_) => Ok(()),
                        };
                        match checked {
                            Ok(()) => NodeStep::ForwardReady(edge),
                            Err(error) => {
                                self.apply_outcome(Outcome::Done(Err(error)), brand, id, anchor)
                            }
                        }
                    }
                    // The producer is not yet resolved: park a checker micro-step on it (an
                    // already-terminal producer never re-notifies, so a park is sound only here). Its
                    // finish runs the declared-return check un-relocated and re-emits `Forward` on a
                    // pass — which re-enters this arm with no ambient obligation (the micro-step ran
                    // obligation-free) and, the producer now resolved, takes the plain `ForwardReady`
                    // path. No re-check, no loop. Both the park and the re-emission name `edge`, this
                    // slot's own name for the producer, which outlives the wait whatever the binder
                    // that first published it does.
                    InstalledEdge::Parked(edge) => {
                        let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
                            // The single parked dep is the producer behind `edge`, delivered
                            // un-relocated at index 0.
                            let producer_terminal = terminals.all()[0];
                            let checked = producer_terminal.cell.open(|value| {
                                check_spliced_return(&obligation, value, view.types())
                            });
                            match checked {
                                Ok(()) => Outcome::Forward(edge),
                                Err(error) => Outcome::Done(Err(error)),
                            }
                        });
                        let park = Outcome::ParkThenContinue {
                            deps: Deps::from_parks([edge]),
                            continuation: Continuation::FinishTerminal(finish),
                            dep_error_frame: Some(dep_error_frame()),
                        };
                        self.apply_outcome(park, brand, id, anchor)
                    }
                }
            }
        }
    }
}

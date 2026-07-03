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
//! `dispatch_body`, `submit_dep_finish_in_own_scope`) — the only callers that turn a `KExpression` into
//! scheduler work.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::kfunction::action::{
    Action, BlockEntry, DepPlacement, FinishCtx, FramePlacement, TailContract,
};
use crate::machine::core::kfunction::body::{
    split_body_statements, ContractFamily, ErasedContract, ReturnContract,
};
use crate::machine::core::kfunction::exec::home_return_type;
use crate::machine::core::ScopeRefFamily;
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::{CallFrame, FrameSet, KError, KErrorKind, NodeId, Scope};
use crate::witnessed::SealedExtern;

use super::dispatch::{BodyPlacement, DepRequest};
use super::lift::relocate_carried;
use super::nodes::{ChainOp, NodePayload, NodeStep, NodeWork};
use super::outcome::{dep_error_frame, Await, Continuation, Outcome};
use super::run_loop::RegionRefFamily;
use super::{
    catch_continuation, ignore_results, relocate_values, seal_witnessed, short_circuit,
    CatchFinish, ContinuationFamily, DepFinish,
};
use crate::machine::model::values::CarriedFamily;
use crate::scheduler::{Deps, ResolvedDeps, Scheduler, Workload};
use crate::witnessed::Witnessed;

mod interpret;
mod submit;

pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};

/// The Koan instantiation of the scheduler's [`Workload`] interface — the marker that binds the four
/// opaque scheduler types to their concrete Koan forms. The scheduler is generic over `W: Workload`
/// and names none of these directly; the workload side (this module, `dispatch/**`) supplies them.
pub(in crate::machine::execute) struct KoanWorkload;

impl Workload for KoanWorkload {
    type Payload = NodePayload;
    type Value = CarriedFamily;
    type Error = KError;
    type Cart = CallFrame;
    type Contract = ContractFamily;
    type Continuation = ContinuationFamily;
    type Witness = FrameSet;
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
    /// The ambient per-step context — the active per-call frame, slot reserve, run frame, the
    /// executing slot's payload, and the contract-chain flag. The scheduler is a pure DAG runtime;
    /// this driver-side state floats across a single step. See [`ambient`](super::ambient).
    pub(in crate::machine::execute) ambient: super::ambient::AmbientContext,
    /// The run lifetime the harness processes its AST/scope against. The scheduler is value-erased
    /// (`Scheduler<KoanWorkload>`), so `'run` lives only in the harness's own method signatures; this
    /// marker keeps it on the type.
    _run: PhantomData<&'run ()>,
}

impl<'run> KoanRuntime<'run> {
    pub fn new() -> Self {
        Self {
            sched: Scheduler::new(),
            ambient: super::ambient::AmbientContext::default(),
            _run: PhantomData,
        }
    }
}

impl Default for KoanRuntime<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Read forwarders to the owned [`Scheduler`]. The harness exposes the scheduler's read surface
/// (terminal reads / slot count) so callers drive the whole run through the harness without ever
/// borrowing the scheduler — the write methods are the inherent `&mut self` ones above.
impl<'run> KoanRuntime<'run> {
    /// Open a slot's terminal at a rank-2 brand and hand the value to `f`, returning its result or
    /// the terminal's error — the destination-verb read. See [`Scheduler::read_result_with`].
    pub fn read_result_with<R>(
        &self,
        id: NodeId,
        f: impl for<'b> FnOnce(crate::machine::model::Carried<'b>) -> R,
    ) -> Result<R, &KError> {
        self.sched.read_result_with(id, f)
    }

    /// A slot terminal's error, or `Ok(())` on success — the value-free probe.
    /// See [`Scheduler::result_error`].
    pub fn result_error(&self, id: NodeId) -> Result<(), &KError> {
        self.sched.result_error(id)
    }

    /// The witness set of a slot's finalized terminal — every region the value reaches. The
    /// test-harness [`extract_terminal`](crate::builtins::test_support) hook for depositing a returned
    /// closure's / module's reach onto a surviving scope's reach-set, mirroring the run-root drain's
    /// `fold_reach`. Production reads the witness off the relocated carrier instead.
    #[cfg(test)]
    pub(crate) fn dep_witness(&self, id: NodeId) -> crate::machine::FrameSet {
        self.sched.dep_witness(id)
    }

    /// Relocate `producer`'s terminal into `dest` and re-seal it under the set union of every region
    /// it reaches and `dest`'s own witness — routing the merge-form
    /// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into), so the relocation re-anchors
    /// with **no fabricated lifetime** at this call site. The spine is copied into `dest` natively at
    /// the merge brand; the surviving closure / module borrows ride the producer's own witness, folded
    /// into the result set by the merge. `dest` arrives as a witnessed carrier — its witness pins its
    /// own backing: the consuming slot's frame for a `Forward`-ready pull (`yoke`d there), the empty
    /// set for the run region a drained root re-homes into (externally pinned).
    ///
    /// This is the storage-bound relocation (`Forward`-ready, drain): the value lands as a re-sealed
    /// [`Witnessed`], not at a step brand. The consumer-pull dep slice does not route this — it opens
    /// in-band at the step brand in [`run_step`](Self::run_step), where the continuation needs the
    /// values live at `'b`.
    pub(in crate::machine::execute) fn relocate_terminal(
        &self,
        producer: NodeId,
        dest: Witnessed<RegionRefFamily, FrameSet>,
    ) -> Result<Witnessed<CarriedFamily, FrameSet>, KError> {
        self.sched
            .transfer_lifted(producer, dest, |value, region, _brand| {
                relocate_carried(value, region)
            })
            .map(|opt| opt.expect("a FrameSet union always represents"))
            .map_err(|e| e.clone())
    }

    pub fn len(&self) -> usize {
        self.sched.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sched.is_empty()
    }
}

/// Test-only forwarders: an immutable `&Scheduler` view (`resolve_name_part` fixtures) plus the
/// AST-free poke surface (`free`, the reserve-reuse counter, a slot's stored chain). No `&mut
/// Scheduler` escapes — the accessor hands out `&Scheduler`, keeping the harness the sole writer.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    pub(in crate::machine::execute) fn scheduler(&self) -> &Scheduler<KoanWorkload> {
        &self.sched
    }

    /// Mutable scheduler access for the white-box scheduler tests that poke `store` / `deps` /
    /// `queues` directly. Test-only — production drives every write through the harness's own
    /// `&mut self` methods, so this is the one sanctioned `&mut Scheduler` outside them.
    pub(in crate::machine::execute) fn scheduler_mut(&mut self) -> &mut Scheduler<KoanWorkload> {
        &mut self.sched
    }

    pub(in crate::machine::execute) fn free(&mut self, idx: usize) {
        self.sched.free(idx)
    }

    pub fn chain_of(&self, id: NodeId) -> Option<Rc<crate::machine::LexicalFrame>> {
        self.sched.payload_of(id).map(|p| p.chain.clone())
    }

    pub fn tail_reuse_count(&self) -> usize {
        self.ambient_tail_reuse_count()
    }
}

/// Lower an [`Action`] into the scheduler's [`Outcome`] currency — a pure `Action -> Outcome`
/// transform that reads nothing: a `AwaitDeps`/`Catch` declares its deps (and a wrapped finish that
/// recurses `run_action` on the `AwaitContinue`/`CatchContinue` it produces) as a [`Outcome::ParkThenContinue`],
/// and the harness submits and applies. Every scheduler read the body needs is deferred into the
/// finish, which sees a read-only [`SchedulerView`](super::dispatch::SchedulerView) at wake.
pub(in crate::machine::execute) fn run_action<'step>(action: Action<'step>) -> Outcome<'step> {
    match action {
        // Terminal: the witnessed carrier (or error) the builtin already computed inside its witness
        // closure (scope was mutated in place first) rides straight through — `finalize` seals it, no
        // asserted-co-location bundle.
        Action::Done(result) => Outcome::Done(result),

        Action::Tail {
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
                return Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: frame_placement,
                    contract,
                    block_entry,
                    body_index,
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
            // `ReuseReserve` mints its cart only at apply time — after the leading statements would
            // already have fanned out — so a leading-carrying tail cannot ride it.
            debug_assert!(
                !matches!(frame_placement, FramePlacement::ReuseReserve { .. }),
                "a leading-carrying tail is a FreshChild frame, an Inherit cart, or an overlay"
            );
            let finish: DepFinish<'step> = Box::new(move |_view, results, _carriers| {
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    // The return-type expression is the last leading statement (all owned), so its
                    // resolved value is the last owned result. The per-call type is re-homed into the
                    // captured-scope region — a strict ancestor the cart keeps live — like the `Type`
                    // form's `PerCall.ret`; a concrete module return type is rejected there (see
                    // `home_return_type`).
                    TailContract::FromLastResult { func } => {
                        let owned = results.owned_slice();
                        let kt = match owned[owned.len() - 1] {
                            Carried::Type(t) => t,
                            Carried::Object(other) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                                    format!(
                                        "FN deferred return-type expression produced a non-type {} value",
                                        other.ktype().name(),
                                    ),
                                ))))
                            }
                        };
                        let ret = match home_return_type(kt, func.captured_scope().brand()) {
                            Ok(ret) => ret,
                            Err(error) => return Outcome::Done(Err(error)),
                        };
                        Some(ReturnContract::PerCall { func, ret })
                    }
                };
                Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: frame_placement,
                    contract,
                    block_entry,
                    body_index,
                }
            });
            Await::on(Deps::from_owned([DepRequest::BodyBlock {
                statements: leading,
                placement,
            }]))
            .error_frame(dep_error_frame())
            .finish(finish)
        }

        Action::AwaitDeps { deps, finish } => {
            // An `Existing` dep is a park-producer the combine reads but doesn't own; every other
            // arm is an owned sub-slot (a builtin only ever declares `Dispatch` — an `InScope` body
            // fans out one per statement at apply time). Parks keep first-occurrence order, owned
            // insertion order; the builder delivers results `[park..., owned...]`. The wrapped
            // finish recurses `run_action` on the `AwaitContinue`.
            let mut built: Deps<DepRequest<'step>> = Deps::new();
            for dep in deps {
                match dep {
                    DepRequest::Existing(id) => {
                        built.park_on(id);
                    }
                    _ => {
                        built.own(dep);
                    }
                }
            }
            let wrapped: DepFinish<'step> = Box::new(move |view, results, _carriers| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    frame: view.dest_frame(),
                };
                run_action(finish(&fctx, results))
            });
            Await::on(built)
                .error_frame(dep_error_frame())
                .finish(wrapped)
        }

        Action::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a dep-finish body's fan-out.
            let wrapped: CatchFinish<'step> = Box::new(move |view, result| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    frame: view.dest_frame(),
                };
                run_action(finish(&fctx, result))
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
        expr: KExpression<'a>,
        placement: DepPlacement<'a>,
    ) -> NodeId {
        match placement {
            DepPlacement::OwnScope => self.dispatch_in_own_scope(expr),
            DepPlacement::InScope(scope) => self
                .enter_block(scope.id, vec![expr], scope)
                .into_iter()
                .next()
                .expect("enter_block of one statement yields one node"),
        }
    }

    /// Realize a [`Catch`](Continuation::Catch)'s single watched [`DepRequest`] to a producer
    /// `NodeId`. `Existing` is already a producer the builtin found in scope; a `Dispatch` realizes as
    /// a single statement (an `InScope` watched expr enters a fresh single-statement block — see
    /// [`Self::realize_dispatch`]). A `Catch` never watches a dispatcher-only lowering.
    fn realize_catch_dep<'a>(&mut self, dep: DepRequest<'a>) -> NodeId {
        match dep {
            DepRequest::Existing(id) => id,
            DepRequest::Dispatch { expr, placement } => self.realize_dispatch(expr, placement),
            DepRequest::ListLit(_)
            | DepRequest::DictLit(_)
            | DepRequest::RecordLit(_)
            | DepRequest::BodyBlock { .. } => {
                unreachable!("a Catch watches only a simple Dispatch/Existing dep")
            }
        }
    }

    /// Resolve a [`FramePlacement`] to the cart a [`Continue`](Outcome::Continue) installs: reuse
    /// the slot's ping-pong reserve (the TCO tail-call cart), take a builtin-minted fresh cart, or
    /// keep the current cart (`None`). The one place the placement → cart mapping lives — shared by
    /// the `Continue` body re-run and the folded invoke / re-resolve paths (which reach it through
    /// their own `Continue`).
    fn resolve_frame_placement<'x>(
        &mut self,
        placement: FramePlacement<'x>,
    ) -> Option<Rc<CallFrame>> {
        match placement {
            FramePlacement::ReuseReserve { outer } => Some(self.acquire_tail_frame(outer)),
            FramePlacement::FreshChild { frame } => Some(frame),
            FramePlacement::Inherit => None,
        }
    }

    /// Reuse the slot's reserve cart (reset in place) when uniquely owned, else mint fresh under
    /// `outer` — the scope-dependent per-call frame construction the scheduler delegates to the
    /// workload. The scheduler owns the reserve *slot* (rotation, lifecycle); this owns the
    /// `CallFrame` minting/reset, which names the run-lived `Scope`. `try_reset_for_tail`'s
    /// `Rc::get_mut` gate is the "no escape" uniqueness check (a cloned `Rc` forecloses reuse).
    fn acquire_tail_frame<'a>(&mut self, outer: &'a Scope<'a>) -> Rc<CallFrame> {
        if let Some(mut reserve) = self.take_active_reserve() {
            if reserve.try_reset_for_tail(outer) {
                self.note_tail_reuse();
                return reserve;
            }
        }
        CallFrame::new(outer, None)
    }

    /// Close the active frame's scope iff this slot owns it: the per-call frame's body has finished
    /// (a `Done` return, or a tail `Continue` retiring this iteration), so the scope
    /// takes no further binds and its reach-set seals. A `Yoked` sub-expression slot owns no frame
    /// (its `owner` never names this slot), so its `Done` is a no-op here.
    fn close_owned_scope(&self, idx: usize) {
        if let Some(frame) = self.ambient.active_frame_ref() {
            if frame.owner() == Some(NodeId(idx)) {
                frame.with_scope(|s| s.close());
            }
        }
    }

    /// Interpret an [`Outcome`] into the scheduler effect it names and return the slot's
    /// [`NodeStep`]. This is the sole graph writer the dispatch side reaches — a decide handler
    /// never holds `&mut Scheduler`.
    pub(in crate::machine::execute) fn apply_outcome<'step>(
        &mut self,
        outcome: Outcome<'step>,
        idx: usize,
    ) -> NodeStep {
        match outcome {
            // The value terminal: a construction carrier already naming its reach rides straight
            // through to the Done boundary, where the workload hook seals it (a declared-return
            // re-stamp aside, untouched). An error carries no value and finalizes bare.
            Outcome::Done(result) => {
                self.close_owned_scope(idx);
                match result {
                    Ok(carrier) => NodeStep::DoneWitnessed(carrier),
                    Err(error) => NodeStep::Error(error),
                }
            }
            Outcome::Continue {
                work,
                frame,
                contract,
                block_entry,
                body_index,
            } => {
                // The body's leading statements are never dispatched here — a producer with leading
                // statements parks on them as owned `BodyBlock` deps and emits this `Continue` only
                // from the resolving finish (see `dispatch/exec.rs` and `run_action`).
                // A tail iteration (`ReuseReserve`) retires this scope before the cart is reused for
                // the next; other placements keep the current scope live.
                if matches!(frame, FramePlacement::ReuseReserve { .. }) {
                    self.close_owned_scope(idx);
                }
                let frame = self.resolve_frame_placement(frame);
                // The body re-dispatched into a freshly installed frame finalizes that frame's scope.
                if let Some(installed) = frame.as_ref() {
                    installed.set_owner(NodeId(idx));
                }
                // Decide the chain reshape from the block scope id + the still-live contract variant,
                // then erase the contract — so the `Replace` step carries no `'run` (the variant is
                // frozen into the lifetime-free [`ChainOp`]). The run loop assembles the chain against
                // the post-step frame and keeps the slot's prior contract first over `contract`. An
                // `Overlay` block entry also rides the tail slot's scope: erased to a cart-witnessed
                // carrier here (where the overlay is still live) so the frameless `Replace` installs
                // it as the slot's `YokedChild` — the frameless analogue of the `Yoked` a framed tail
                // re-projects from its own cart.
                let (block_scope_id, overlay_scope) = match block_entry {
                    BlockEntry::None => (None, None),
                    BlockEntry::FrameScope(frame) => (Some(frame.scope_id()), None),
                    BlockEntry::Overlay(overlay) => (
                        Some(overlay.id),
                        Some(SealedExtern::<ScopeRefFamily>::erase(overlay)),
                    ),
                };
                let chain = ChainOp::decide(block_scope_id, contract.as_ref(), body_index);
                NodeStep::Replace {
                    work,
                    frame,
                    contract: contract.map(ErasedContract::erase),
                    chain,
                    overlay_scope,
                }
            }
            Outcome::ParkThenContinue {
                deps,
                continuation,
                dep_error_frame,
            } => {
                // Realize the builder's owned requests into producer ids, rebuilding a
                // `ResolvedDeps` from the same parks. An `Existing` request realizes to itself; an
                // `InScope`-placed `Dispatch` and a `BodyBlock` each fan out to one owned producer
                // per statement (so those arms `own` per id, the rest own one). Parks keep their
                // first-occurrence order, owned their realization order — the `[park..., owned...]`
                // delivery order a finish addresses through [`DepResults`].
                let (parks, owned_requests) = deps.into_parts();
                let mut resolved = ResolvedDeps::from_parks(parks);
                for dep in owned_requests {
                    match dep {
                        // An `InScope` body fans out one producer per statement (multi-statement
                        // split); `OwnScope` realizes as a single producer via the shared
                        // [`Self::realize_dispatch`].
                        DepRequest::Dispatch {
                            expr,
                            placement: DepPlacement::InScope(scope),
                        } => {
                            let statements = split_body_statements(expr);
                            for id in self.enter_block(scope.id, statements, scope) {
                                resolved.own(id);
                            }
                        }
                        DepRequest::Dispatch { expr, placement } => {
                            resolved.own(self.realize_dispatch(expr, placement));
                        }
                        DepRequest::ListLit(items) => {
                            resolved.own(self.schedule_list_literal(items));
                        }
                        DepRequest::DictLit(pairs) => {
                            resolved.own(self.schedule_dict_literal(pairs));
                        }
                        DepRequest::RecordLit(fields) => {
                            resolved.own(self.schedule_record_literal(fields));
                        }
                        // A body block fans out one owned producer per statement: into a fresh
                        // per-call frame's own scope (`dispatch_body`), or — under `Inherit` — into a
                        // caller-allocated overlay via the same `enter_block` fan-out the leading
                        // statements of an `InScope` body use (USING).
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Frame(frame),
                        } => {
                            for id in self.dispatch_body(&frame, statements) {
                                resolved.own(id);
                            }
                        }
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Overlay(overlay),
                        } => {
                            for id in self.enter_block(overlay.id, statements, overlay) {
                                resolved.own(id);
                            }
                        }
                        DepRequest::Existing(id) => {
                            resolved.own(id);
                        }
                    }
                }
                // Install the resolved list's edges against this slot: each park a `Notify` edge
                // (kept alive), each owned dep an `Owned` edge (cascade-freed on resolve). (`Catch`
                // declares no deps here, so `resolved` is empty — it realizes and owns its single
                // watched dep in the `cont` match below.)
                self.sched.install_edges(&resolved, NodeId(idx));
                let work = match continuation {
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal dep-finish carries the `dep_error_frame()`
                    // label. Both install the same `Wait` over the realized deps (edges already
                    // installed above), the short-circuit baked into the continuation by
                    // `short_circuit` — the one loop, `relocate_values` its value-copy projection.
                    Continuation::Finish(finish) => NodeWork::new(
                        resolved,
                        short_circuit(dep_error_frame, relocate_values(finish)),
                        None,
                    ),
                    // The construction-inversion sibling: same realized deps and edges, but the
                    // `seal_witnessed` projection folds the resolved terminals (value + reach) into
                    // one witnessed carrier and seals as `Done(Ok)`.
                    Continuation::FinishWitnessed(finish) => NodeWork::new(
                        resolved,
                        short_circuit(dep_error_frame, seal_witnessed(finish)),
                        None,
                    ),
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a dep-finish body's fan-out); realize and own it here.
                    // `catch_continuation` runs the finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let from = self.realize_catch_dep(watched);
                        self.sched.add_owned_edge(from, NodeId(idx));
                        let mut watched_deps = ResolvedDeps::new();
                        watched_deps.own(from);
                        NodeWork::new(watched_deps, catch_continuation(finish), None)
                    }
                    // The resume closure carries the evolving `working_expr` from here on; the
                    // `carrier` it travels with is only the deadlock-summary sample. A decide takes
                    // no dep values, so `ignore_results` drops the (park-only) results view.
                    Continuation::Resume { carrier, resume } => {
                        NodeWork::new(resolved, ignore_results(resume), carrier)
                    }
                };
                NodeStep::Replace {
                    work,
                    frame: None,
                    contract: None,
                    chain: ChainOp::Unchanged,
                    overlay_scope: None,
                }
            }
            Outcome::Forward(producer) => {
                // The slot's result *is* `producer`'s. If `producer` is ready, finalize the slot by
                // pulling its terminal into this slot's own frame (the consumer-pull lift — the
                // producer keeps its value in its own frame, which frees out from under a bare copy),
                // then this slot's consumers pull from here. Otherwise splice the slot out: move its
                // consumers onto `producer`'s notify list and alias the slot to `producer`.
                if self.sched.is_result_ready(producer) {
                    // The forwarded terminal *is* this slot's; `run_step` relocates it into this
                    // slot's region carrying its own witness (the forwarded terminal already enforced
                    // its own contract, so no re-check). `Alias` is the not-ready twin below.
                    NodeStep::ForwardReady(producer)
                } else {
                    // Not ready: `NodeStep::Alias` drives `splice_forward` (move consumers onto the
                    // producer + alias the slot) in the execute loop.
                    NodeStep::Alias(producer)
                }
            }
        }
    }
}

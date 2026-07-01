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
    Action, BlockEntry, Dep, DepPlacement, FinishCtx, FramePlacement,
};
use crate::machine::core::kfunction::body::{
    split_body_statements, ContractFamily, ErasedContract,
};
use crate::machine::core::ScopeRefFamily;
use crate::machine::model::ast::KExpression;
use crate::machine::{CallFrame, FrameSet, KError, NodeId, Scope};
use crate::witnessed::SealedExtern;

use super::dispatch::{BodyPlacement, DepRequest};
use super::lift::relocate_carried;
use super::nodes::{ChainOp, NodePayload, NodeStep, NodeWork};
use super::outcome::{dep_error_frame, Continuation, Outcome};
use super::run_loop::RegionRefFamily;
use super::{
    catch_continuation, ignore_results, short_circuit, short_circuit_witnessed, CatchFinish,
    ContinuationFamily, DepFinish,
};
use crate::machine::model::values::CarriedFamily;
use crate::scheduler::{Scheduler, Workload};
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
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => Outcome::Done(Ok(c)),
        Action::Done(Err(e)) => Outcome::Done(Err(e)),
        // Object-family terminal: the carrier the builtin built inside its witness closure rides
        // straight through — `finalize` seals it, no asserted-co-location bundle.
        Action::DoneWitnessed(carrier) => Outcome::DoneWitnessed(carrier),

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
                return Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: frame_placement,
                    contract,
                    block_entry,
                    body_index,
                };
            }
            // Leading statements become owned siblings in the block (one `BodyBlock` dep); the slot
            // parks on them so they run — and cascade-free — before the tail continues, keeping the
            // side-effect order and (for a fresh frame) the uniqueness TCO reuse needs. Two block
            // shapes carry leading statements: a `FreshChild` frame whose own scope is the block
            // (MATCH / TRY arms — fan out via `dispatch_body`), or an `Inherit` overlay entered
            // without a fresh frame (USING — fan out via `enter_block`). The tail re-enters the same
            // block: a `FreshChild` re-installs the frame, an `Inherit` keeps the call-site cart.
            let overlay = match &block_entry {
                BlockEntry::Overlay(scope) => Some(*scope),
                _ => None,
            };
            let (body_block, continue_frame): (DepRequest<'step>, FramePlacement<'step>) =
                match frame_placement {
                    FramePlacement::FreshChild { frame } => {
                        let body_frame = frame.clone();
                        (
                            DepRequest::BodyBlock {
                                statements: leading,
                                placement: BodyPlacement::Frame(frame),
                            },
                            FramePlacement::FreshChild { frame: body_frame },
                        )
                    }
                    FramePlacement::Inherit => {
                        let overlay = overlay.expect(
                            "a leading-carrying Inherit tail carries an overlay block (USING)",
                        );
                        (
                            DepRequest::BodyBlock {
                                statements: leading,
                                placement: BodyPlacement::Overlay(overlay),
                            },
                            FramePlacement::Inherit,
                        )
                    }
                    FramePlacement::ReuseReserve { .. } => unreachable!(
                        "a leading-carrying tail is a FreshChild frame or an Inherit overlay"
                    ),
                };
            let finish: DepFinish<'step> =
                Box::new(move |_view, _results, _carriers| Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: continue_frame,
                    contract,
                    block_entry,
                    body_index,
                });
            Outcome::ParkThenContinue {
                deps: vec![body_block],
                park_count: 0,
                continuation: Continuation::Finish(finish),
                dep_error_frame: Some(dep_error_frame()),
            }
        }

        Action::AwaitDeps { deps, finish } => {
            // `Existing` deps are park-producers the combine reads but doesn't own; `Dispatch`
            // deps are owned sub-slots (an `InScope` body fans out one per statement at apply
            // time). The harness orders the realized deps `[park..., owned...]`; `park_count` is
            // the park prefix length. The wrapped finish recurses `run_action` on the `AwaitContinue`.
            let mut park: Vec<DepRequest<'step>> = Vec::new();
            let mut owned: Vec<DepRequest<'step>> = Vec::new();
            for dep in deps {
                match dep {
                    Dep::Existing(id) => park.push(DepRequest::Existing(id)),
                    Dep::Dispatch { expr, placement } => {
                        owned.push(DepRequest::Dispatch { expr, placement })
                    }
                }
            }
            let park_count = park.len();
            park.extend(owned);
            let wrapped: DepFinish<'step> = Box::new(move |view, results, _carriers| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                };
                run_action(finish(&fctx, results))
            });
            Outcome::ParkThenContinue {
                deps: park,
                park_count,
                continuation: Continuation::Finish(wrapped),
                dep_error_frame: Some(dep_error_frame()),
            }
        }

        Action::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a dep-finish body's fan-out.
            let wrapped: CatchFinish<'step> = Box::new(move |view, result| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                };
                run_action(finish(&fctx, result))
            });
            Outcome::ParkThenContinue {
                deps: Vec::new(),
                park_count: 0,
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

    /// Realize a [`Catch`](Continuation::Catch)'s single watched [`Dep`] to a producer `NodeId`.
    /// `Existing` is already a producer the builtin found in scope; a `Dispatch` realizes as a
    /// single statement (an `InScope` watched expr enters a fresh single-statement block — see
    /// [`Self::realize_dispatch`]).
    fn realize_catch_dep<'a>(&mut self, dep: Dep<'a>) -> NodeId {
        match dep {
            Dep::Existing(id) => id,
            Dep::Dispatch { expr, placement } => self.realize_dispatch(expr, placement),
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
    /// (a `Done` / `DoneWitnessed` return, or a tail `Continue` retiring this iteration), so the scope
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
            // A bare terminal. A value is region-pure relative to its producer frame (a builtin's
            // direct `Action::Done(Ok)` — its args resolved synchronously, nothing born reaching a
            // dep region): seal it region-pure through `resident` (born under the empty set, the
            // producer frame folded in at finalize/close) so it joins the sole witnessed value
            // terminal. An error carries no value and finalizes bare.
            Outcome::Done(Ok(value)) => {
                self.close_owned_scope(idx);
                NodeStep::DoneWitnessed(Witnessed::<CarriedFamily, FrameSet>::resident(value))
            }
            Outcome::Done(Err(error)) => {
                self.close_owned_scope(idx);
                NodeStep::Error(error)
            }
            // A construction carrier rides straight through to the Done boundary, where the workload
            // hook seals it (a declared-return re-stamp aside, untouched).
            Outcome::DoneWitnessed(carrier) => {
                self.close_owned_scope(idx);
                NodeStep::DoneWitnessed(carrier)
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
                    BlockEntry::FrameScope(id) => (Some(id), None),
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
                park_count,
                continuation,
                dep_error_frame,
            } => {
                // Submit each fresh dep (an `Existing` is already in the graph). Submission order
                // is preserved, so a finish reads `results[k]` for the k-th declared dep — except
                // an `InScope`-placed `Dispatch` and a `BodyBlock`, whose multi-statement body each
                // fan out to one producer per statement (so those arms `extend`, the rest `push`).
                let mut dep_ids: Vec<NodeId> = Vec::with_capacity(deps.len());
                for dep in deps {
                    match dep {
                        // An `InScope` body fans out one producer per statement (multi-statement
                        // split); `OwnScope` realizes as a single producer via the shared
                        // [`Self::realize_dispatch`].
                        DepRequest::Dispatch {
                            expr,
                            placement: DepPlacement::InScope(scope),
                        } => {
                            let statements = split_body_statements(expr);
                            dep_ids.extend(self.enter_block(scope.id, statements, scope))
                        }
                        DepRequest::Dispatch { expr, placement } => {
                            dep_ids.push(self.realize_dispatch(expr, placement))
                        }
                        DepRequest::ListLit(items) => {
                            dep_ids.push(self.schedule_list_literal(items))
                        }
                        DepRequest::DictLit(pairs) => {
                            dep_ids.push(self.schedule_dict_literal(pairs))
                        }
                        DepRequest::RecordLit(fields) => {
                            dep_ids.push(self.schedule_record_literal(fields))
                        }
                        // A body block fans out one owned producer per statement: into a fresh
                        // per-call frame's own scope (`dispatch_body`), or — under `Inherit` — into a
                        // caller-allocated overlay via the same `enter_block` fan-out the leading
                        // statements of an `InScope` body use (USING).
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Frame(frame),
                        } => dep_ids.extend(self.dispatch_body(&frame, statements)),
                        DepRequest::BodyBlock {
                            statements,
                            placement: BodyPlacement::Overlay(overlay),
                        } => dep_ids.extend(self.enter_block(overlay.id, statements, overlay)),
                        DepRequest::Existing(id) => dep_ids.push(id),
                    }
                }
                // Edge install: the `[..park_count]` prefix is notify-parked (sibling producers
                // the slot waits on but doesn't own); the `[park_count..]` suffix is owned
                // (cascade-freed on resolve). Each continuation sets `park_count` to match: a
                // dispatch `Finish` owns all its deps (`park_count: 0`); an action `AwaitDeps` parks
                // its `Existing` prefix and owns its `Dispatch` suffix; `Replay` parks every
                // producer (`park_count: len`); a bare-name `Forward` parks its one producer
                // (`park_count: 1`) while a deferred-combine `Forward` owns it (`park_count: 0`).
                // (`Catch` declares no deps here — it realizes and owns its single watched dep in
                // the `cont` match below.)
                for (i, id) in dep_ids.iter().enumerate() {
                    if i < park_count {
                        self.sched.add_park_edge(*id, NodeId(idx));
                    } else {
                        self.sched.add_owned_edge(*id, NodeId(idx));
                    }
                }
                let work = match continuation {
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal dep-finish carries the `dep_error_frame()`
                    // label. Both install the same `Wait` over the realized deps (edges already
                    // installed by the loop above), the short-circuit baked into the continuation by
                    // `short_circuit`.
                    Continuation::Finish(finish) => NodeWork::new(
                        dep_ids,
                        park_count,
                        short_circuit(dep_error_frame, finish),
                        None,
                    ),
                    // The construction-inversion sibling: same realized deps and edges, but the
                    // continuation folds the resolved terminals (value + reach) into one witnessed
                    // carrier and seals as `DoneWitnessed` (see [`short_circuit_witnessed`]).
                    Continuation::FinishWitnessed(finish) => NodeWork::new(
                        dep_ids,
                        park_count,
                        short_circuit_witnessed(dep_error_frame, finish),
                        None,
                    ),
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a dep-finish body's fan-out); realize and own it here.
                    // `catch_continuation` runs the finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let from = self.realize_catch_dep(watched);
                        self.sched.add_owned_edge(from, NodeId(idx));
                        NodeWork::new(vec![from], 0, catch_continuation(finish), None)
                    }
                    // The resume closure carries the evolving `working_expr` from here on; the
                    // `carrier` it travels with is only the deadlock-summary sample. A decide takes
                    // no dep values, so `ignore_results` drops the (park-only) results slice.
                    Continuation::Resume { carrier, resume } => {
                        NodeWork::new(dep_ids, park_count, ignore_results(resume), carrier)
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

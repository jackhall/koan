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
//! [`interpret_with_writer_path`]); they parse, stand up the arena/root scope, and drive the run via
//! [`KoanRuntime::run_program`]. The [`submit`] submodule holds the AST-aware dispatch-submission
//! wrappers ([`KoanRuntime::enter_block`], [`KoanRuntime::dispatch_in_scope`], `dispatch_in_own_scope`,
//! `dispatch_body`, `submit_dep_finish_in_own_scope`) — the only callers that turn a `KExpression` into
//! scheduler work.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::kfunction::action::{
    Action, Dep, DepPlacement, FinishCtx, FramePlacement,
};
use crate::machine::core::kfunction::body::{
    split_body_statements, ContractFamily, ErasedContract,
};
use crate::machine::model::ast::KExpression;
use crate::machine::{CallArena, KError, NodeId, Scope};

use super::dispatch::{reattach_node_scope, DepRequest};
use super::lift::NodeLift;
use super::nodes::{ChainOp, NodePayload, NodeStep, NodeWork};
use super::outcome::{dep_error_frame, Continuation, Outcome};
use super::{
    catch_continuation, ignore_results, short_circuit, CatchFinish, ContinuationFamily, DepFinish,
};
use crate::machine::model::values::CarriedFamily;
use crate::scheduler::{reattach_value, Scheduler, Workload};

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
    type Frame = CallArena;
    type Contract = ContractFamily;
    type Continuation = ContinuationFamily;
}

/// The write harness: the sole holder of `&mut Scheduler` across the execute tree. It owns the
/// [`Scheduler`] by composition (a `sched` field, not a `&mut` borrow) and carries every AST-aware
/// and graph-mutating step — the execute loop, [`Self::apply_outcome`], the dispatch-submission
/// wrappers, `submit_dispatch`, and the literal lowering. A dispatch *decide* runs against a
/// read-only [`SchedulerView`](super::dispatch::SchedulerView) over `&self.sched` and returns an
/// [`Outcome`]; only the harness reborrows the scheduler mutably to apply it. So "everything outside
/// the harness is read-only" is structurally enforced, not a naming convention.
///
/// See design/execution-model.md § the dispatcher / scheduler boundary.
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
    /// Read a slot's terminal, re-anchored by the scheduler to this `&self` borrow (the slot's frame
    /// `Rc` pins the value for the borrow), so the driver-side read is safe. See
    /// [`Scheduler::read_result`].
    pub fn read_result(&self, id: NodeId) -> Result<crate::machine::model::Carried<'_>, &KError> {
        self.sched.read_result(id)
    }

    /// Read a slot's value terminal, panicking on `Err`. See [`Scheduler::read`].
    pub fn read(&self, id: NodeId) -> crate::machine::model::Carried<'_> {
        self.sched.read(id)
    }

    /// Pull a dep's terminal lifted into `dest` (consumer-pull): a framed terminal is copied out of
    /// its producer frame, a frameless / run-arena terminal is forwarded as-is. The `unsafe` reattach
    /// is internal — the slot's co-stored frame `Rc` / run arena pins the value for the transient
    /// read, and the lift copies it into `dest` — so the run loop's dep collection stays safe.
    pub(in crate::machine::execute) fn read_lifted<'o>(
        &self,
        dep: NodeId,
        dest: &'o crate::machine::core::RuntimeArena,
    ) -> Result<crate::machine::model::Carried<'o>, KError> {
        match self.sched.read_result_with_frame(dep) {
            // Re-anchor the scheduler's `'node` read to the destination *node* lifetime `'o` (the
            // consumer scope's arena, bounded by the held consumer-frame `Rc`), then lift it into
            // `dest`. The held producer frame `Rc` witnesses the framed re-anchor (the lift
            // self-anchors the copy via the embedded `Rc`); a frameless terminal lives in a run arena
            // that outlives `'o`. Node-scale — no `'run` fabrication.
            // SAFETY: `'node` and `'o` are both pinned for the read+lift by the held producer frame
            // `Rc` (Some) / the run arena (None); the carrier is invariant, so the lifetime-only
            // re-anchor routes `reattach_value`.
            Ok((value, Some(frame))) => Ok(self.lift(
                unsafe { reattach_value::<CarriedFamily>(value) },
                &frame,
                dest,
            )),
            Ok((value, None)) => Ok(unsafe { reattach_value::<CarriedFamily>(value) }),
            Err(e) => Err(e.clone()),
        }
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
/// recurses `run_action` on the `AwaitContinue`/`CatchCont` it produces) as a [`Outcome::ParkThenContinue`],
/// and the harness submits and applies. Every scheduler read the body needs is deferred into the
/// finish, which sees a read-only [`SchedulerView`](super::dispatch::SchedulerView) at wake.
pub(in crate::machine::execute) fn run_action<'step>(action: Action<'step>) -> Outcome<'step> {
    match action {
        // Terminal: the value the builtin already computed (scope was mutated in place first).
        Action::Done(Ok(c)) => Outcome::Done(Ok(c)),
        Action::Done(Err(e)) => Outcome::Done(Err(e)),

        Action::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        } => {
            // A block-entering tail sits above the params (`1`) or the leading siblings (`N`); a
            // frameless continuation keeps the slot's block at index `0`.
            let body_index = if block_entry.is_some() {
                leading.len() + 1
            } else {
                0
            };
            if leading.is_empty() {
                // No leading statements: tail-replace directly into the arm body.
                return Outcome::Continue {
                    work: super::dispatch::decide(tail),
                    frame: frame_placement,
                    contract,
                    block_entry,
                    body_index,
                };
            }
            // Leading statements become owned siblings in the arm's frame (one `BodyBlock` dep);
            // the slot parks on them so they run — and cascade-free — before the tail continues,
            // keeping the side-effect order and the frame uniqueness TCO reuse needs. An arm that
            // carries leading statements always mints its own `FreshChild` frame (`branch_walk`),
            // so the placement resolves to a concrete cart here without a scheduler write.
            let frame = match frame_placement {
                FramePlacement::FreshChild { frame } => frame,
                _ => unreachable!(
                    "an action Tail with leading statements always carries a FreshChild frame"
                ),
            };
            let body_frame = frame.clone();
            let finish: DepFinish<'step> = Box::new(move |_view, _results| Outcome::Continue {
                work: super::dispatch::decide(tail),
                frame: FramePlacement::FreshChild { frame: body_frame },
                contract,
                block_entry,
                body_index,
            });
            Outcome::ParkThenContinue {
                deps: vec![DepRequest::BodyBlock {
                    frame,
                    statements: leading,
                }],
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
            let wrapped: DepFinish<'step> = Box::new(move |view, results| {
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
    /// re-dispatches against the executing slot's own scope; `ActiveFrame` inherits the ambient
    /// per-call frame; `InScope` enters a fresh **single-statement** block (so an inner `LET` stays
    /// local). A multi-statement body splits separately — see the `InScope` arm of [`Self::apply_outcome`]
    /// and [`Self::dispatch_body`].
    fn realize_dispatch<'a>(
        &mut self,
        expr: KExpression<'a>,
        placement: DepPlacement<'a>,
    ) -> NodeId {
        match placement {
            DepPlacement::OwnScope => self.dispatch_in_own_scope(expr),
            DepPlacement::ActiveFrame => {
                let chain = self.ambient_or_detached_chain();
                self.dispatch_in_active_frame(expr, chain)
            }
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
    ) -> Option<Rc<CallArena>> {
        match placement {
            FramePlacement::ReuseReserve { outer } => Some(self.acquire_tail_frame(outer)),
            FramePlacement::FreshChild { frame } => Some(frame),
            FramePlacement::Inherit => None,
        }
    }

    /// Reuse the slot's reserve cart (reset in place) when uniquely owned, else mint fresh under
    /// `outer` — the scope-dependent per-call frame construction the scheduler delegates to the
    /// workload. The scheduler owns the reserve *slot* (rotation, lifecycle); this owns the
    /// `CallArena` minting/reset, which names the run-lived `Scope`. `try_reset_for_tail`'s
    /// `Rc::get_mut` gate is the "no escape" uniqueness check (a cloned `Rc` forecloses reuse).
    fn acquire_tail_frame(&mut self, outer: &Scope<'_>) -> Rc<CallArena> {
        if let Some(mut reserve) = self.take_active_reserve() {
            if reserve.try_reset_for_tail(outer) {
                self.note_tail_reuse();
                return reserve;
            }
        }
        CallArena::new(outer, None)
    }

    /// Interpret an [`Outcome`] into the scheduler effect it names and return the slot's
    /// [`NodeStep`]. This is the sole graph writer the dispatch side reaches — a decide handler
    /// never holds `&mut Scheduler`.
    pub(in crate::machine::execute) fn apply_outcome<'step>(
        &mut self,
        outcome: Outcome<'step>,
        idx: usize,
    ) -> NodeStep<'step> {
        match outcome {
            // The terminal stays at the step lifetime `'s` — the run loop's `run_step` finalizes it
            // into the slot store (erasing it) before the step's frame witness drops, so it never
            // needs a fabricated `'run` to cross the step-guard exit.
            Outcome::Done(output) => NodeStep::Done(output),
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
                let frame = self.resolve_frame_placement(frame);
                // Decide the chain reshape from the still-live contract variant, then erase the
                // contract — so the `Replace` step carries no `'run` (the variant is frozen into the
                // lifetime-free [`ChainOp`]). The run loop assembles the chain against the post-step
                // frame and keeps the slot's prior contract first over `contract`.
                let chain = ChainOp::decide(block_entry, contract.as_ref(), body_index);
                NodeStep::Replace {
                    work,
                    frame,
                    contract: contract.map(ErasedContract::erase),
                    chain,
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
                        // split); `OwnScope` / `ActiveFrame` realize as a single producer via the
                        // shared [`Self::realize_dispatch`].
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
                        DepRequest::BodyBlock { frame, statements } => {
                            dep_ids.extend(self.dispatch_body(&frame, statements))
                        }
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
                }
            }
            Outcome::Forward(producer) => {
                // The slot's result *is* `producer`'s. If `producer` is ready, finalize the slot by
                // pulling its terminal into this slot's own frame (the consumer-pull lift — the
                // producer keeps its value in its own frame, which frees out from under a bare copy),
                // then this slot's consumers pull from here. Otherwise splice the slot out: move its
                // consumers onto `producer`'s notify list and alias the slot to `producer`.
                if self.sched.is_result_ready(producer) {
                    // Pull `producer`'s terminal into this consumer's own scope arena, at a *node*
                    // lifetime bounded by the active cart — not a fabricated `'run`. `read_lifted`
                    // does the same node-scale re-anchor the dep path uses: it lifts a framed
                    // terminal into `dest` and forwards a frameless one as-is, with no `'run` step.
                    let payload = self
                        .ambient
                        .active_payload()
                        .expect("a slot step installs the ambient payload");
                    let dest =
                        reattach_node_scope(&payload.scope, self.ambient.active_frame_ref()).arena;
                    let pulled = self.read_lifted(producer, dest);
                    // Shorten the node value to the uniform `NodeStep` step lifetime `'s`: it lives
                    // in `dest`, which the active cart pins for all of `'s`. A node→step reattach,
                    // not a `'run` fabrication.
                    // SAFETY: the value lives in `dest` ⊇ `'s`; lifetime-only reattach of an
                    // invariant carrier.
                    NodeStep::Done(pulled.map(|v| unsafe { reattach_value::<CarriedFamily>(v) }))
                } else {
                    // Not ready: `NodeStep::Alias` drives `splice_forward` (move consumers onto the
                    // producer + alias the slot) in the execute loop.
                    NodeStep::Alias(producer)
                }
            }
        }
    }
}

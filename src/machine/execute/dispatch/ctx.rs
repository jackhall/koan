//! Read-only dispatch view.
//!
//! [`SchedulerView`] is the surface every dispatch *decide* runs against: it holds `&Scheduler`
//! (never `&mut`) for its reads — the static-over-the-step ones (`current_scope`, `chain_deref`,
//! …) and the live reads of *pre-existing* producers (`is_result_ready`, `would_create_cycle`,
//! `read_result`) — and the decide *returns* a
//! [`Outcome`](super::Outcome) the [`harness`](super::runtime) applies.
//! [`KoanRuntime`](super::runtime::KoanRuntime) owns the scheduler and is the sole holder of `&mut
//! Scheduler` across the execute tree, so no decide handler touches it — the scheduler's write
//! primitives are inherent methods the harness alone calls.
//!
//! The dispatcher genuinely reads evolving graph state, so full scheduler-unawareness (the builtin
//! model) is not a goal — only the *writes* defer to the harness. Dispatch *shape* modules
//! (`keyworded`, `fn_value`, `single_poll`) never name scheduler fields directly — only
//! `cx.foo(...)` — so a future scheduler internal rename is a single-file change inside `scheduler/`.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::kfunction::action::DepPlacement;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Carried;
use crate::machine::{CallArena, KError, LexicalFrame, NameOutcome, NodeId, Scope};

use super::super::ambient::AmbientContext;
use super::super::nodes::NodeScope;
use super::super::runtime::KoanWorkload;
use super::{park_on_deps, resolve_name_part, DepRequest, Outcome, PendingSub};
use crate::scheduler::Scheduler;

/// Re-anchor a raw [`NodeScope`] handle into a usable `&Scope` — the Koan scope interpretation the
/// scheduler no longer owns. The driver hands back the opaque payload
/// ([`AmbientContext::active_payload`] / `PostStep::payload`), from which the workload extracts the
/// scope handle, plus the per-call cart the slot ran against; this workload-side helper reattaches
/// them. An `Anchored` slot reattaches its erased
/// run-lived [`ScopePtr`](crate::machine::core::ScopePtr) (`reattach_bounded`); a `Yoked` slot
/// re-projects from `frame`. Content lifetime free, borrow bounded by `frame` — so the result
/// cannot outlive the cart it names.
pub(in crate::machine::execute) fn reattach_node_scope<'step, 'b: 'step>(
    node_scope: &'step NodeScope,
    frame: Option<&'step Rc<CallArena>>,
) -> &'step Scope<'b> {
    match node_scope {
        // SAFETY: the `Anchored` pointer was erased from a genuinely run-lived scope
        // (`resolve_node_scope`), which outlives `frame`; the returned borrow is bounded by `frame`,
        // so the free content lifetime cannot be cashed past the run-lived pointee.
        NodeScope::Anchored(ptr) => unsafe { ptr.reattach_bounded() },
        NodeScope::Yoked => frame
            .expect("a Yoked slot keeps its active cart")
            .scope_bounded(),
    }
}

/// The active slot's scope, re-anchored from the ambient payload's scope handle. The workload-side
/// form of the read the scheduler core no longer owns: it materializes a `&Scope` so `scheduler/**`
/// names none. Panics outside a slot step (no ambient payload); within a step the scope is always
/// present — an `Anchored` slot carries its own pointer, and a `Yoked` slot's active cart is never
/// emptied mid-step (an invoke reuses the reserve, not the active cart).
pub(in crate::machine::execute) fn current_scope<'run>(ambient: &AmbientContext) -> &Scope<'run> {
    let payload = ambient
        .active_payload()
        .expect("a slot step installs the ambient payload (and a Yoked slot keeps its frame)");
    reattach_node_scope(&payload.scope, ambient.active_frame_ref())
}

/// Read-only dispatch view — the decide-phase context. It holds only `&Scheduler`, never `&mut`.
/// A shape handler decides against this and *returns* a
/// [`Outcome`](super::Outcome); the harness reborrows the scheduler
/// mutably to apply the writes. The borrow contract: a `SchedulerView` lives only for the decide
/// call, the handler returns an owned outcome, and the immutable borrow ends before the harness
/// takes `&mut` — so decide and apply never overlap.
pub(in crate::machine::execute) struct SchedulerView<'run, 's> {
    sched: &'s Scheduler<KoanWorkload>,
    /// The driver's ambient per-step context: the scope/chain reads (`current_scope`, `chain_deref`,
    /// `active_chain`, `current_frame`, `in_contract_chain`) read it, not the scheduler.
    ambient: &'s AmbientContext,
    /// `SchedulerView` re-anchors the value-erased scheduler's reads to `'run` (the AST/scope
    /// lifetime the decide runs against); the scheduler itself is `Scheduler<KoanWorkload>`, so
    /// `'run` lives only on this view, kept here by the marker.
    _run: PhantomData<&'run ()>,
}

impl<'run, 's> SchedulerView<'run, 's> {
    pub(in crate::machine::execute) fn new(
        sched: &'s Scheduler<KoanWorkload>,
        ambient: &'s AmbientContext,
    ) -> Self {
        Self {
            sched,
            ambient,
            _run: PhantomData,
        }
    }

    // Read surface (forwards on `&self`) — the static-over-the-step reads (`current_scope`,
    // `chain_deref`, `active_chain`) and the live reads of pre-existing producers
    // (`is_result_ready`, `would_create_cycle`, `read_result`) all forward to the borrowed
    // scheduler.

    pub(in crate::machine::execute) fn current_scope(&self) -> &Scope<'run> {
        current_scope(self.ambient)
    }

    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.ambient.active_payload().map(|p| &*p.chain)
    }

    /// Cloned `Rc` to the active chain — the type-leaf and field-list reads that take the
    /// chain by value.
    pub(super) fn active_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.ambient.active_payload().map(|p| p.chain.clone())
    }

    /// Cloned `Rc` to the active lexical chain — the `record_type` elaborator deferral needs
    /// it by value.
    pub(super) fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.ambient.active_payload().map(|p| p.chain.clone())
    }

    /// Cloned `Rc` to the active per-call frame — the `invoke` decide reads it to build a
    /// builtin's `BodyCtx`. `None` only outside any frame (top-level builtins).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.ambient.active_frame_ref().cloned()
    }

    /// Whether the executing slot already carries a kept return contract (a tail call within an
    /// established chain) — `invoke` reads it so a deferred-return FN skips re-resolving its
    /// keep-first-discarded return type.
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.ambient.active_in_contract_chain
    }

    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        self.sched.is_result_ready(id)
    }

    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        // SAFETY: the slot's co-stored frame Rc / run arena pins the value; read is transient.
        self.sched.read_result(id).map(|v| unsafe { v.reattach() })
    }

    pub(super) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.sched.would_create_cycle(producer, consumer)
    }

    /// Build the per-part `bare_outcomes` cache: one `resolve_name_part` per bare-name part,
    /// `None` otherwise. `consumer = None` defers cycle detection to the splice walk.
    pub(super) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<ExpressionPart<'run>>],
    ) -> Vec<Option<NameOutcome<'run>>> {
        let active_chain = self.ambient.active_payload().map(|p| &p.chain);
        parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => Some(resolve_name_part(
                    self.current_scope(),
                    &p.value,
                    self.sched,
                    active_chain,
                    None,
                )),
                _ => None,
            })
            .collect()
    }

    /// Stage each `PendingSub` and decide the eager-subs outcome. A `Reuse` of an already-resolved
    /// producer splices inline (a read of a static-over-this-step slot) and rides on the outcome's
    /// `free`; a freshly minted sub is never terminal in the same step, so it becomes an owned
    /// `AwaitDeps` dep. The finish splices the resolved values into `working_expr` and routes on
    /// `picked` — `Some(f)` folds the committed call into a frame-installing `Continue`, `None`
    /// re-resolves via [`keyworded::finish`](super::keyworded::finish). When every sub spliced
    /// inline, that routing happens now; otherwise the slot parks as a `AwaitDeps` and the routing
    /// runs in the finish. The `<bind>` dep-error frame rides on `dep_error_frame`. Read-only —
    /// every write the outcome implies is the harness's.
    pub(super) fn install_eager_subs(
        &self,
        mut working_expr: KExpression<'run>,
        staged_subs: Vec<(usize, PendingSub<'run>)>,
        picked: Option<&'run KFunction<'run>>,
    ) -> Outcome<'run, 'run> {
        use super::super::DepFinish;
        let mut deps: Vec<DepRequest<'run>> = Vec::with_capacity(staged_subs.len());
        let mut part_indices: Vec<usize> = Vec::with_capacity(staged_subs.len());
        for (i, pending) in staged_subs {
            // Every sub is delivered through the single consumer-pull path: a `Reuse` parks on its
            // pre-existing producer as an `Existing` dep (a ready one is a late parker the pull-lift
            // serves), a freshly-staged sub is a fresh dep the harness submits. No value is read and
            // spliced inline at decide time — that would embed a producer's frame-local terminal,
            // which its per-call frame does not keep alive past the frame's own free (a producer
            // holds its terminal in-frame and never lifts at Done), so the reference would dangle.
            let dep = match pending {
                PendingSub::Reuse(id) => DepRequest::Existing(id),
                PendingSub::Dispatch(sub_expr) => DepRequest::Dispatch {
                    expr: sub_expr,
                    placement: DepPlacement::OwnScope,
                },
                PendingSub::ListLit(items) => DepRequest::ListLit(items),
                PendingSub::DictLit(pairs) => DepRequest::DictLit(pairs),
                PendingSub::RecordLit(fields) => DepRequest::RecordLit(fields),
            };
            deps.push(dep);
            part_indices.push(i);
        }
        if deps.is_empty() {
            // No subs to resolve — `working_expr` is already fully resolved, so route to the finish
            // now instead of parking on a dep-finish.
            return finish_eager_subs(working_expr, picked);
        }
        let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
            "<bind>",
            &working_expr,
        ));
        let finish: DepFinish<'run> = Box::new(move |_ctx, results| {
            // The short-circuit already guaranteed every dep resolved; splice each into the slot it
            // was staged from, then route the continuation. `results` are the dep terminals,
            // pull-lifted into this node's frame and re-exposed at `'run` by the combinator, so they
            // splice straight into the `'run` working expression that re-dispatches in this frame.
            for (slot, value) in part_indices.iter().zip(results) {
                working_expr.parts[*slot].value = ExpressionPart::Future(*value);
            }
            finish_eager_subs(working_expr, picked)
        });
        park_on_deps(deps, dep_error_frame, finish)
    }
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation — the shared tail of
/// the `AwaitDeps` finish and its all-inline fast path. `Some(f)` folds the committed call into a
/// frame-installing [`Outcome::Continue`] (via [`invoke_continue`](super::exec::invoke_continue));
/// `None` defers to a re-resolve `Continue` (via
/// [`redispatch_continue`](super::keyworded::redispatch_continue), which re-runs
/// [`keyworded::finish`](super::keyworded::finish), where an element-typed `Future(_)` revealed by a
/// sub surfaces as a slot-terminal `DispatchFailed`). Pure data — no `&mut`.
fn finish_eager_subs<'run>(
    working_expr: KExpression<'run>,
    picked: Option<&'run KFunction<'run>>,
) -> Outcome<'run, 'run> {
    match picked {
        Some(f) => super::exec::invoke_continue(f, working_expr),
        None => super::keyworded::redispatch_continue(working_expr),
    }
}

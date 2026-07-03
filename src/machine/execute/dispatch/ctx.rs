//! Read-only dispatch view.
//!
//! [`SchedulerView`] is the surface every dispatch *decide* runs against: it holds `&Scheduler`
//! (never `&mut`) for its reads — the static-over-the-step ones (`current_scope`, `chain_deref`,
//! …) and the live reads of *pre-existing* producers (`producer_disposition`, `would_create_cycle`,
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

use std::rc::Rc;

use crate::machine::core::kfunction::action::{scope_frame, DepPlacement};
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::FrameStorage;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::values::CarriedFamily;
use crate::machine::{CallFrame, FrameSet, KError, LexicalFrame, NameOutcome, NodeId, Scope};
use crate::source::Spanned;
use crate::witnessed::{Sealed, StepContext};

use super::super::ambient::AmbientContext;
use super::super::nodes::NodeScope;
use super::super::runtime::KoanWorkload;
use super::{resolve_name_part, Await, DepRequest, Outcome, PendingSub};
use crate::scheduler::{Deps, ProducerDisposition, Scheduler};

/// Run `f` with a raw [`NodeScope`] handle's scope opened at a `for<'b>` brand — the Koan scope
/// interpretation the scheduler does not own, folded onto `open` like the decide channel. The driver
/// hands back the opaque payload ([`AmbientContext::active_payload`]), from which the workload extracts
/// the scope handle, plus the per-call cart the slot ran against. A `Yoked` slot re-projects from the
/// active cart through [`CallFrame::with_scope`] (the same `open` the step brand uses); a `YokedChild`
/// slot opens its erased cart-ancestor [`SealedExtern<ScopeRefFamily>`](crate::witnessed::SealedExtern)
/// carrier at the same `for<'b>` brand, pinned by `frame` — the scope folded onto the holder's own
/// `open` like every other channel. Either way the `&Scope<'b>` is confined to `f`, so no borrow rides
/// up a `&mut self` path.
pub(in crate::machine::execute) fn with_node_scope<R>(
    node_scope: &NodeScope,
    frame: Option<&Rc<CallFrame>>,
    f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
) -> R {
    let frame = frame.expect("a slot keeps its active cart");
    match node_scope {
        NodeScope::YokedChild(carrier) => carrier.open(frame, f),
        NodeScope::Yoked => frame.with_scope(f),
    }
}

/// Run `f` with the active slot's scope, opened at a `for<'b>` brand from the ambient payload's scope
/// handle — the read the `&mut self` literal-classify and submit paths use (they hold `self.ambient`,
/// not the step `open`'s branded scope). Panics outside a slot step; within a step the scope is always
/// present — a `YokedChild` slot carries its own pointer, and a `Yoked` slot's active cart is never
/// emptied mid-step.
pub(in crate::machine::execute) fn with_current_node_scope<R>(
    ambient: &AmbientContext,
    f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
) -> R {
    let payload = ambient
        .active_payload()
        .expect("a slot step installs the ambient payload (and a Yoked slot keeps its frame)");
    with_node_scope(&payload.scope, ambient.active_frame_ref(), f)
}

/// The frame storage owning the active slot's scope region, read through the ambient payload — the
/// `&mut self` classify path's analogue of [`SchedulerView::dest_frame`]. Routes `scope_frame`, the
/// liveness invariant's single owner.
pub(in crate::machine::execute) fn current_dest_frame(
    ambient: &AmbientContext,
) -> Rc<FrameStorage> {
    with_current_node_scope(ambient, scope_frame)
}

/// Read-only dispatch view — the decide-phase context. It holds only `&Scheduler`, never `&mut`.
/// A shape handler decides against this and *returns* a
/// [`Outcome`](super::Outcome); the harness reborrows the scheduler
/// mutably to apply the writes. The borrow contract: a `SchedulerView` lives only for the decide
/// call, the handler returns an owned outcome, and the immutable borrow ends before the harness
/// takes `&mut` — so decide and apply never overlap.
pub(in crate::machine::execute) struct SchedulerView<'step, 'view> {
    sched: &'view Scheduler<KoanWorkload>,
    /// The driver's ambient per-step context: the scope/chain reads (`current_scope`, `chain_deref`,
    /// `active_chain`, `current_frame`, `in_contract_chain`) read it, not the scheduler.
    ambient: &'view AmbientContext,
    /// The active slot's scope, opened at the step brand and handed in by the run-loop step `open`
    /// (`run_step`), so [`Self::current_scope`] returns it directly rather than re-anchoring an erased
    /// handle up the dispatcher stack. It carries the cart/scope content lifetime `'step` the decide
    /// runs at: every decide runs at the cart lifetime, the working expression re-anchored from its
    /// erased node carrier to `'step`, so the view's slot is the cart, never the program AST. The
    /// pristine-AST lifetime `'ast` lives only at the submission boundary, where a borrowed
    /// `&KExpression<'ast>` is read against the cart scope.
    scope: &'step Scope<'step>,
    /// The `Rc<FrameStorage>` owning the active scope's region — resolved once per step by the run
    /// loop (via `scope_frame`, the invariant's single owner) while the step machinery holds it, so
    /// step code reads a live frame with no failure path.
    dest_frame: Rc<FrameStorage>,
}

impl<'step, 'view> SchedulerView<'step, 'view> {
    pub(in crate::machine::execute) fn new(
        sched: &'view Scheduler<KoanWorkload>,
        ambient: &'view AmbientContext,
        scope: &'step Scope<'step>,
        dest_frame: Rc<FrameStorage>,
    ) -> Self {
        Self {
            sched,
            ambient,
            scope,
            dest_frame,
        }
    }

    // Read surface (forwards on `&self`) — the static-over-the-step reads (`current_scope`,
    // `chain_deref`, `active_chain`) and the live reads of pre-existing producers
    // (`would_create_cycle`, `producer_disposition`, `read_result`) all forward to the borrowed
    // scheduler.

    /// Run `f` with the active slot's scope. The scope was opened at the step brand and handed to this
    /// view, so it satisfies the `for<'b>` closure at the view's own `'step`; the closure form is kept
    /// for the handlers that consume their scope in place, alongside the plain [`Self::current_scope`].
    pub(in crate::machine::execute) fn with_current_scope<R>(
        &self,
        f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
    ) -> R {
        f(self.scope)
    }

    pub(in crate::machine::execute) fn current_scope(&self) -> &'step Scope<'step> {
        self.scope
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
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallFrame>> {
        self.ambient.active_frame_ref().cloned()
    }

    /// The frame storage owning the active scope's region — infallible: resolved at step entry from
    /// what the step machinery already holds. The destination frame for in-step allocation
    /// (`alloc_witnessed` / `yoke_branded`) and relocation.
    pub(in crate::machine::execute) fn dest_frame(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.dest_frame)
    }

    /// The step construction context wrapping [`Self::dest_frame`] — the library-owned
    /// `ctx.region()` / `ctx.alloc()` / `ctx.alloc_with()` surface (`design/scheduler-library.md`
    /// guarantees 3 and 5), handed to a finish through
    /// [`FinishCtx`](crate::machine::core::kfunction::action::FinishCtx).
    pub(in crate::machine::execute) fn step_ctx(&self) -> StepContext<FrameStorage> {
        StepContext::new(self.dest_frame())
    }

    /// Whether the executing slot already carries a kept return contract (a tail call within an
    /// established chain) — `invoke` reads it so a deferred-return FN skips re-resolving its
    /// keep-first-discarded return type.
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.ambient.active_in_contract_chain
    }

    pub(super) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.sched.would_create_cycle(producer, consumer)
    }

    /// Classify whether this slot can depend on `producer` — the shared park ladder (ready → errored
    /// → would-cycle → park). `consumer` is `None` at a leaf-park site with no consumer id in scope,
    /// where a cycle can never be classified. Each caller keeps its own policy per arm.
    pub(super) fn producer_disposition(
        &self,
        producer: NodeId,
        consumer: Option<NodeId>,
    ) -> ProducerDisposition<'_, KError> {
        self.sched.producer_disposition(producer, consumer)
    }

    /// Build the per-part `bare_outcomes` cache: one `resolve_name_part` per bare-name part,
    /// `None` otherwise. `consumer = None` defers cycle detection to the splice walk.
    pub(super) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<ExpressionPart<'step>>],
    ) -> Vec<Option<NameOutcome<'step>>> {
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
        mut working_expr: KExpression<'step>,
        staged_subs: Vec<(usize, PendingSub<'step>)>,
        picked: Option<&'step KFunction<'step>>,
        inline_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
    ) -> Outcome<'step> {
        use super::super::DepFinish;
        let mut deps: Vec<DepRequest<'step>> = Vec::with_capacity(staged_subs.len());
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
            // now instead of parking on a dep-finish. Only the inline-resolved wrap slots carry a
            // reach carrier here; a scalar-literal arg is region-pure ("no foreign reach").
            return finish_eager_subs(working_expr, picked, inline_carriers);
        }
        let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
            "<bind>",
            &working_expr,
        ));
        let finish: DepFinish<'step> = Box::new(move |_ctx, values, carriers| {
            // The short-circuit already guaranteed every dep resolved; splice each value into the slot
            // it was staged from, and collect each dep's carrier (keyed by that slot) to deliver to the
            // body. `values` are pull-lifted into this node's frame and re-exposed at `'step`, so they
            // splice straight into the `'step` working expression that re-dispatches here; `carriers`
            // name each arg's reach and ride on (a `duplicate` per arg — the producer keeps its seal)
            // to `run_action_builtin` / the user-fn arg fold.
            // Start from the inline-resolved wrap slots' carriers and add each staged sub's carrier,
            // so the body receives every value arg's reach (inline plus eager-sub).
            let mut arg_carriers = inline_carriers;
            arg_carriers.reserve(part_indices.len());
            // Every eager sub is an owned dep, so its result lands in the owned suffix in staging
            // order — 1:1 with `part_indices`.
            for ((slot, value), carrier) in part_indices
                .iter()
                .zip(values.owned_slice())
                .zip(carriers.owned_slice())
            {
                working_expr.parts[*slot].value = ExpressionPart::Spliced(*value);
                arg_carriers.push((*slot, carrier.duplicate()));
            }
            finish_eager_subs(working_expr, picked, arg_carriers)
        });
        Await::on(Deps::from_owned(deps))
            .error_frame(dep_error_frame)
            .finish(finish)
    }
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation — the shared tail of
/// the `AwaitDeps` finish and its all-inline fast path. `Some(f)` folds the committed call into a
/// frame-installing [`Outcome::Continue`] (via [`invoke_continue`](super::exec::invoke_continue));
/// `None` defers to a re-resolve `Continue` (via
/// [`redispatch_continue`](super::keyworded::redispatch_continue), which re-runs
/// [`keyworded::finish`](super::keyworded::finish), where an element-typed `Spliced(_)` revealed by a
/// sub surfaces as a slot-terminal `DispatchFailed`). Pure data — no `&mut`.
fn finish_eager_subs<'step>(
    working_expr: KExpression<'step>,
    picked: Option<&'step KFunction<'step>>,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    match picked {
        Some(f) => super::exec::invoke_continue(f, working_expr, arg_carriers),
        // The re-resolve path commits its call in `keyworded::finish`; thread the arg carriers through
        // so the re-resolved builtin / user-fn still receives every value arg's reach.
        None => super::keyworded::redispatch_continue(working_expr, arg_carriers),
    }
}

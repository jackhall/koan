//! Read-only dispatch view.
//!
//! [`SchedulerView`] is the surface every dispatch *decide* runs against: it holds `&Scheduler`
//! (never `&mut`) and *returns* an [`Outcome`](super::Outcome) the [`harness`](super::runtime)
//! applies. [`KoanRuntime`](super::runtime::KoanRuntime) is the sole holder of `&mut Scheduler`, so
//! no decide handler touches the scheduler's write primitives.
//!
//! Dispatch reads evolving graph state, so scheduler-unawareness is not a goal — only the *writes*
//! defer to the harness. Dispatch shape modules (`keyworded`, `fn_value`, `single_poll`) reach the
//! scheduler only through `cx.foo(...)`, never by naming its fields, so a scheduler-internal rename
//! stays inside `scheduler/`.

use std::rc::Rc;

use crate::machine::core::KFunction;
use crate::machine::core::{scope_frame, DepPlacement};
use crate::machine::core::{FrameStorage, StepAllocator};
use crate::machine::model::types::TypeRegistry;
use crate::machine::model::FoldDirection;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::{CallFrame, KError, LexicalFrame, NameOutcome, NodeId, Scope};
use crate::source::{Span, Spanned};

use super::super::ambient::AmbientContext;
use super::super::nodes::NodeScope;
use super::super::obligation::ReturnObligation;
use super::super::runtime::KoanWorkload;
use super::{resolve_name_part, Await, DepRequest, Outcome, PendingSub};
use crate::scheduler::{Deps, ProducerDisposition, Scheduler};

/// Run `f` with a [`NodeScope`] handle's scope opened at a `for<'b>` brand. A `Yoked` slot
/// re-projects from the active cart through [`CallFrame::with_scope`]; a `YokedChild` slot opens its
/// erased cart-ancestor [`SealedExtern<ScopeRefFamily>`](crate::witnessed::SealedExtern) carrier at
/// the same brand, pinned by `frame`. Either way the `&Scope<'b>` is confined to `f`, so no borrow
/// rides up a `&mut self` path.
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

/// Run `f` with the active slot's scope from the ambient payload — the read the `&mut self`
/// literal-classify and submit paths use (they hold `self.ambient`, not the step's branded scope).
/// Panics outside a slot step; within a step the scope is always present.
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
/// `&mut self` classify path's analogue of [`SchedulerView::dest_frame`]. Routes through
/// `scope_frame`, the liveness invariant's single owner.
pub(in crate::machine::execute) fn current_dest_frame(
    ambient: &AmbientContext,
) -> Rc<FrameStorage> {
    with_current_node_scope(ambient, scope_frame)
}

/// Read-only dispatch view — the decide-phase context, holding only `&Scheduler`. A shape handler
/// decides against this and returns an [`Outcome`](super::Outcome); the harness then reborrows the
/// scheduler mutably to apply the writes. The borrow contract: a `SchedulerView` lives only for the
/// decide call and the immutable borrow ends before the harness takes `&mut`, so decide and apply
/// never overlap.
pub(in crate::machine::execute) struct SchedulerView<'step, 'view> {
    sched: &'view Scheduler<KoanWorkload>,
    /// Per-step context for the scope/chain reads (`current_scope`, `chain_deref`, `active_chain`,
    /// `current_frame`, `in_contract_chain`), which read it rather than the scheduler.
    ambient: &'view AmbientContext,
    /// The active slot's scope, opened at the step brand and handed in by the run-loop step `open`,
    /// so [`Self::current_scope`] returns it directly. It carries the cart content lifetime `'step`
    /// every decide runs at; the pristine-AST lifetime `'ast` lives only at the submission boundary,
    /// where a borrowed `&KExpression<'ast>` is read against the cart scope.
    scope: &'step Scope<'step>,
    /// The `Rc<FrameStorage>` owning the active scope's region — resolved once per step by the run
    /// loop while the step machinery holds it, so step code reads a live frame with no failure path.
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

    /// Run `f` with the active slot's scope. The closure form is for handlers that consume their
    /// scope in place, alongside the plain [`Self::current_scope`].
    pub(in crate::machine::execute) fn with_current_scope<R>(
        &self,
        f: impl for<'b> FnOnce(&'b Scope<'b>) -> R,
    ) -> R {
        f(self.scope)
    }

    pub(in crate::machine::execute) fn current_scope(&self) -> &'step Scope<'step> {
        self.scope
    }

    /// The run's subtype-verdict store, read through the ambient context's run frame. Memoized
    /// predicates take it as their final parameter.
    pub(in crate::machine::execute) fn types(&self) -> &TypeRegistry {
        self.ambient.type_registry()
    }

    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.ambient.active_payload().map(|p| &*p.chain)
    }

    /// Cloned `Rc` to the active chain — for the type-leaf and field-list reads that take it by value.
    pub(super) fn active_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.ambient.active_payload().map(|p| p.chain.clone())
    }

    /// Cloned `Rc` to the active lexical chain — the `record_type` elaborator deferral needs it by
    /// value.
    pub(super) fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.ambient.active_payload().map(|p| p.chain.clone())
    }

    /// Cloned `Rc` to the active per-call frame. `None` only outside any frame (top-level builtins).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallFrame>> {
        self.ambient.active_frame_ref().cloned()
    }

    /// The frame storage owning the active scope's region — infallible: resolved at step entry from
    /// what the step machinery already holds. The destination frame for in-step allocation
    /// (`alloc_witnessed` / `yoke_branded`) and relocation.
    pub(in crate::machine::execute) fn dest_frame(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.dest_frame)
    }

    /// The step construction allocator wrapping [`Self::dest_frame`], branded at the step lifetime
    /// `'step` — its doors return a [`StepCarried`](crate::machine::execute::StepCarried) confined to
    /// the step (`design/scheduler-library.md` guarantees 3 and 5), handed to a finish through
    /// [`FinishCtx`](crate::machine::core::FinishCtx).
    pub(in crate::machine::execute) fn step_ctx(&self) -> StepAllocator<'step> {
        StepAllocator::over_frame(self.dest_frame())
    }

    /// Whether the executing slot already carries a kept return contract (a tail call within an
    /// established chain) — `invoke` reads it so a deferred-return FN skips re-resolving its
    /// keep-first-discarded return type.
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.ambient.in_contract_chain()
    }

    /// Deposit the slot's declared-return obligation into the ambient slot-step state — the reach
    /// the [`with_obligation`](super::super::obligation::with_obligation) wrapper closure runs to
    /// carry the checker down the tail chain.
    pub(in crate::machine::execute) fn deposit_obligation(&self, obligation: ReturnObligation) {
        self.ambient.deposit_obligation(obligation)
    }

    /// Duplicate the chain's established obligation without removing it — keep-first and park
    /// propagation read it to wrap the replacement continuation.
    pub(in crate::machine::execute) fn current_obligation_duplicate(
        &self,
    ) -> Option<ReturnObligation> {
        self.ambient.current_obligation_duplicate()
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
                    self.types(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Stage each `PendingSub` as a dep and decide the eager-subs outcome. Every sub becomes an
    /// `AwaitDeps` dep — a `Reuse` an `Existing` edge on its pre-existing producer, a fresh sub an
    /// owned edge the harness submits (see the loop for why nothing is spliced inline at decide
    /// time). The finish splices the resolved carriers into `working_expr` and routes on `picked`:
    /// `Some(f)` folds the committed call into a frame-installing `Continue`, `None` re-resolves via
    /// [`keyworded::finish`](super::keyworded::finish). With no subs, that routing happens now. The
    /// `<bind>` dep-error frame rides on `dep_error_frame`. Read-only — every write the outcome
    /// implies is the harness's.
    pub(super) fn install_eager_subs(
        &self,
        mut working_expr: KExpression<'step>,
        staged_subs: Vec<(usize, PendingSub<'step>)>,
        picked: Option<&'step KFunction<'step>>,
    ) -> Outcome<'step> {
        use super::super::TerminalDepFinish;
        let mut deps: Vec<DepRequest<'step>> = Vec::with_capacity(staged_subs.len());
        let mut part_indices: Vec<usize> = Vec::with_capacity(staged_subs.len());
        for (i, pending) in staged_subs {
            // Every sub is pulled through the single consumer path: a `Reuse` parks on its
            // pre-existing producer as an `Existing` dep, a fresh sub is a dep the harness submits.
            // Nothing is read and spliced inline here — that would embed a producer's frame-local
            // terminal, which its per-call frame frees at Done (it never lifts), so it would dangle.
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
            // Nothing to resolve — `working_expr` is already fully spliced, so route now not park.
            return finish_eager_subs(self, working_expr, picked);
        }
        let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
            "<bind>",
            &working_expr,
        ));
        let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
            // Every dep resolved. Splice each value into its staged slot as the producer's own sealed
            // carrier — value and reach as one unit, adopted by the consuming bind at its own step
            // brand; `invoke` reads each cell back for the body-facing reach. Owned deps land in the
            // owned suffix in staging order — 1:1 with `part_indices`.
            for (slot, terminal) in part_indices.iter().zip(terminals.owned_slice()) {
                // Duplicate the dep's delivery envelope — its carrier bundled with the retained
                // producer-frame owner — so the value's backing stays retained across the `Replace`
                // to the re-dispatch step where `extract_carried_args` adopts it. A frameless / run
                // producer carries a `None` host inside the envelope, its backing outliving the cell.
                working_expr.parts[*slot].value = ExpressionPart::Spliced {
                    cell: terminal.delivered.duplicate(),
                };
            }
            finish_eager_subs(ctx, working_expr, picked)
        });
        Await::on(Deps::from_owned(deps))
            .error_frame(dep_error_frame)
            .finish_terminal(finish)
    }

    /// Stage every `Pairwise`-mode operand as its own owned dep, then — once all of them
    /// resolve — build the run's pair-tree and dispatch it. Unlike [`Self::install_eager_subs`]
    /// (which splices each resolved cell back into the *original* expression's own slot, one
    /// destination apiece), a pairwise run's shared middle operands each feed **two** adjacent
    /// pairs (`f x < g y < h z` evaluates `g y` once, its cell duplicated into both the `x<y`
    /// and `y<z` pairs), so this finish builds an entirely fresh pair-tree rather than routing
    /// back through `finish_eager_subs`.
    ///
    /// `operands`/`operators` are the chain's own parts in source order
    /// (`operators.len() == operands.len() - 1`); `is_operator_chain_shape` guarantees at least
    /// 5 parts, so there are always at least 3 operands / 2 operators / 2 pairs — the
    /// combiner-fold loop below always runs at least once. `combiner` is the group's combiner
    /// symbol and `direction` its declared fold (see [`combine`](super::operator_chain::combine)
    /// for the synthesized shape); `chain_span` labels the synthesized combiner parts, which have
    /// no single source token of their own.
    pub(super) fn install_pairwise_fold(
        &self,
        operands: Vec<Spanned<ExpressionPart<'step>>>,
        operators: Vec<Spanned<ExpressionPart<'step>>>,
        combiner: String,
        direction: FoldDirection,
        chain_span: Option<Span>,
        dep_error_frame: Option<crate::machine::TraceFrame>,
    ) -> Outcome<'step> {
        use super::super::TerminalDepFinish;
        use super::operator_chain::combine;

        let operand_spans: Vec<Option<Span>> =
            operands.iter().map(|operand| operand.span).collect();
        let deps: Vec<DepRequest<'step>> = operands
            .into_iter()
            .map(|operand| DepRequest::Dispatch {
                expr: KExpression::new(vec![operand]),
                placement: DepPlacement::OwnScope,
            })
            .collect();
        let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
            // Every operand resolved. Build one pair per operator, duplicating each shared
            // middle operand's resolved cell into both of the adjacent pairs it feeds — the
            // splice that makes evaluation once-only.
            let cells = terminals.owned_slice();
            let mut pairs = Vec::with_capacity(operators.len());
            for (i, operator) in operators.into_iter().enumerate() {
                let left = Spanned {
                    value: ExpressionPart::Spliced {
                        cell: cells[i].delivered.duplicate(),
                    },
                    span: operand_spans[i],
                };
                let right = Spanned {
                    value: ExpressionPart::Spliced {
                        cell: cells[i + 1].delivered.duplicate(),
                    },
                    span: operand_spans[i + 1],
                };
                pairs.push(KExpression::new(vec![left, operator, right]));
            }
            // Fold the pairs through the combiner in the declared direction, nesting exactly like
            // `reduce_fold_left` / `reduce_fold_right`'s accumulator loops.
            let acc = match direction {
                FoldDirection::Left => {
                    let mut pairs = pairs.into_iter();
                    let mut acc = pairs.next().expect(PAIRWISE_HAS_TWO_PAIRS);
                    for pair in pairs {
                        acc = combine(&combiner, acc, pair, chain_span);
                    }
                    acc
                }
                FoldDirection::Right => {
                    let mut pairs = pairs.into_iter().rev();
                    let mut acc = pairs.next().expect(PAIRWISE_HAS_TWO_PAIRS);
                    for pair in pairs {
                        acc = combine(&combiner, pair, acc, chain_span);
                    }
                    acc
                }
            };
            super::become_dispatch(ctx, acc)
        });
        Await::on(Deps::from_owned(deps))
            .error_frame(dep_error_frame)
            .finish_terminal(finish)
    }
}

/// A pairwise run has one pair per operator and the chain shape guarantees ≥2 operators, so the
/// pair list the combiner fold consumes is never empty.
const PAIRWISE_HAS_TWO_PAIRS: &str =
    "pairwise always has ≥2 pairs (chain shape guarantees ≥2 operators)";

/// Route a fully-spliced eager-subs `working_expr` to its continuation. `Some(f)` folds the
/// committed call into a frame-installing `Continue` via
/// [`invoke_continue`](super::exec::invoke_continue); `None` re-resolves via
/// [`redispatch_continue`](super::keyworded::redispatch_continue), which re-runs
/// [`keyworded::finish`](super::keyworded::finish) — there an element-typed `Spliced(_)` revealed by
/// a sub surfaces as a slot-terminal `DispatchFailed`. Pure data — no `&mut`.
fn finish_eager_subs<'step>(
    view: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
    picked: Option<&'step KFunction<'step>>,
) -> Outcome<'step> {
    match picked {
        Some(f) => super::exec::invoke_continue(view, f, working_expr),
        None => super::keyworded::redispatch_continue(view, working_expr),
    }
}

//! Read-only dispatch view.
//!
//! [`SchedulerView`] is the surface every dispatch *decide* runs against: it holds `&Scheduler`
//! (never `&mut`) for its reads — the static-over-the-step ones (`current_scope`, `chain_deref`,
//! …) and the live reads of *pre-existing* producers (`is_result_ready`, `would_create_cycle`,
//! `read_result`) — and the decide *returns* a
//! [`Outcome`](super::Outcome) the [`harness`](super::harness) applies.
//! The harness holds the only `&mut Scheduler` on the dispatch side, so no decide handler touches
//! it — the scheduler's write primitives are inherent methods the harness alone calls.
//!
//! The dispatcher genuinely reads evolving graph state, so full scheduler-unawareness (the builtin
//! model) is not a goal — only the *writes* defer to the harness. Dispatch *shape* modules
//! (`keyworded`, `fn_value`, `single_poll`) never name scheduler fields directly — only
//! `cx.foo(...)` — so a future scheduler internal rename is a single-file change inside `scheduler/`.

use std::rc::Rc;

use crate::machine::core::kfunction::action::DepPlacement;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Carried;
use crate::machine::{CallArena, KError, LexicalFrame, NameOutcome, NodeId, Scope};

use super::super::scheduler::Scheduler;
use super::{bind_frame_err, park_combine, resolve_name_part, DispatchDep, Outcome, PendingSub};

/// Read-only dispatch view — the decide-phase context. It holds only `&Scheduler`, never `&mut`.
/// A shape handler decides against this and *returns* a
/// [`Outcome`](super::Outcome); the harness reborrows the scheduler
/// mutably to apply the writes. The borrow contract: a `SchedulerView` lives only for the decide
/// call, the handler returns an owned outcome, and the immutable borrow ends before the harness
/// takes `&mut` — so decide and apply never overlap.
pub(in crate::machine::execute) struct SchedulerView<'run, 's> {
    sched: &'s Scheduler<'run>,
}

impl<'run, 's> SchedulerView<'run, 's> {
    pub(in crate::machine::execute) fn new(sched: &'s Scheduler<'run>) -> Self {
        Self { sched }
    }

    // Read surface (forwards on `&self`) — the static-over-the-step reads (`current_scope`,
    // `chain_deref`, `active_chain`) and the live reads of pre-existing producers
    // (`is_result_ready`, `would_create_cycle`, `read_result`) all forward to the borrowed
    // scheduler.

    pub(in crate::machine::execute) fn current_scope(&self) -> &Scope<'run> {
        self.sched.current_scope()
    }

    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.sched.chain_deref()
    }

    /// Cloned `Rc` to the active chain — the type-leaf and field-list reads that take the
    /// chain by value.
    pub(super) fn active_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.sched.active_chain_clone()
    }

    /// Cloned `Rc` to the active lexical chain — the `record_type` elaborator deferral needs
    /// it by value.
    pub(super) fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.sched.current_lexical_chain()
    }

    /// Cloned `Rc` to the active per-call frame — the `invoke` decide reads it to build a
    /// builtin's `BodyCtx`. `None` only outside any frame (top-level builtins).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.sched.current_frame()
    }

    /// Whether the executing slot already carries a kept return contract (a tail call within an
    /// established chain) — `invoke` reads it so a deferred-return FN skips re-resolving its
    /// keep-first-discarded return type.
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.sched.in_contract_chain()
    }

    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        self.sched.is_result_ready(id)
    }

    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        self.sched.read_result(id)
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
        parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => Some(resolve_name_part(
                    self.current_scope(),
                    &p.value,
                    self.sched,
                    None,
                )),
                _ => None,
            })
            .collect()
    }

    /// Stage each `PendingSub` and decide the eager-subs outcome. A `Reuse` of an already-resolved
    /// producer splices inline (a read of a static-over-this-step slot) and rides on the outcome's
    /// `free`; a freshly minted sub is never terminal in the same step, so it becomes an owned
    /// `Combine` dep. The finish splices the resolved values into `working_expr` and routes on
    /// `picked` — `Some(f)` calls `f` ([`Outcome::Invoke`]), `None` re-resolves
    /// ([`Outcome::Redispatch`] → [`keyworded::finish`](super::keyworded::finish)). When every sub spliced
    /// inline, that routing happens now; otherwise the slot parks as a `Combine` and the routing
    /// runs in the finish. The `<bind>` dep-error frame rides on `dep_error_frame`. Read-only —
    /// every write the outcome implies is the harness's.
    pub(super) fn install_eager_subs(
        &self,
        mut working_expr: KExpression<'run>,
        staged_subs: Vec<(usize, PendingSub<'run>)>,
        picked: Option<&'run KFunction<'run>>,
    ) -> Outcome<'run> {
        use super::super::CombineFinish;
        let mut deps: Vec<DispatchDep<'run>> = Vec::with_capacity(staged_subs.len());
        let mut part_indices: Vec<usize> = Vec::with_capacity(staged_subs.len());
        // Reuse producers consumed inline (spliced into `working_expr`); the harness reclaims
        // them so the decide phase issues no `free` write.
        let mut free: Vec<usize> = Vec::new();
        for (i, pending) in staged_subs {
            // A `Reuse` is a pre-existing producer the pre-pick found: splice it inline if it has
            // already resolved (a read of a static-over-this-step slot), else park on it as an
            // `Existing` dep. A freshly-staged sub (`Dispatch`/`*Lit`) is never terminal in the
            // same step (submission is enqueue-then-drain), so it is always a fresh dep the harness
            // submits — never read back here.
            let dep = match pending {
                PendingSub::Reuse(id) => {
                    if self.is_result_ready(id) {
                        match self.read_result(id) {
                            Err(e) => return bind_frame_err(e, &working_expr),
                            Ok(value) => {
                                working_expr.parts[i].value = ExpressionPart::Future(value);
                                free.push(id.index());
                                continue;
                            }
                        }
                    }
                    DispatchDep::Existing(id)
                }
                PendingSub::Dispatch(sub_expr) => DispatchDep::Dispatch {
                    expr: sub_expr,
                    placement: DepPlacement::OwnScope,
                },
                PendingSub::ListLit(items) => DispatchDep::ListLit(items),
                PendingSub::DictLit(pairs) => DispatchDep::DictLit(pairs),
                PendingSub::RecordLit(fields) => DispatchDep::RecordLit(fields),
            };
            deps.push(dep);
            part_indices.push(i);
        }
        if deps.is_empty() {
            // Every sub was an already-resolved `Reuse` spliced inline — `working_expr` is fully
            // resolved, so route to the finish now instead of parking on a Combine; the inline
            // frees ride on the resulting Invoke/Redispatch outcome.
            return finish_eager_subs(working_expr, picked, free);
        }
        let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
            "<bind>",
            &working_expr,
        ));
        let finish: CombineFinish<'run> = Box::new(move |_ctx, results| {
            // The short-circuit already guaranteed every dep resolved; splice each into the
            // slot it was staged from, then route the continuation. No inline frees remain at
            // wake — those were drained when the Combine was installed.
            for (slot, value) in part_indices.iter().zip(results) {
                working_expr.parts[*slot].value = ExpressionPart::Future(*value);
            }
            finish_eager_subs(working_expr, picked, Vec::new())
        });
        park_combine(deps, dep_error_frame, finish, free)
    }
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation — the shared tail of
/// the `Combine` finish and its all-inline fast path. `Some(f)` names the committed
/// call as an [`Outcome::Invoke`]; `None` defers to a [`Outcome::Redispatch`]
/// (the harness re-resolves via [`keyworded::finish`](super::keyworded::finish), where an element-typed `Future(_)`
/// revealed by a sub surfaces as a slot-terminal `DispatchFailed`). Pure data — no `&mut`.
fn finish_eager_subs<'run>(
    working_expr: KExpression<'run>,
    picked: Option<&'run KFunction<'run>>,
    free: Vec<usize>,
) -> Outcome<'run> {
    match picked {
        Some(f) => Outcome::Invoke {
            picked: f,
            working_expr,
            free,
        },
        None => Outcome::Redispatch { working_expr, free },
    }
}

//! Dispatch shape router, classifier, and shared spine.
//!
//! [`run_dispatch`] classifies the slot via [`classify_dispatch_shape`]
//! and routes by shape:
//!
//! - **Keyworded** (a keyword is present) → [`keyworded::KeywordedState`]
//! - **FunctionValueCall** (lowercase Identifier head) →
//!   [`fn_value::FnValueState`]
//! - **HeadDeferred** / **TypeHeadDeferred** (an `Expression` or `:(…)`
//!   head that evaluates before dispatching on its result) →
//!   [`head_deferred`]
//! - **OperatorChain** → [`operator_chain`]
//! - **TypeCall**, **BareIdentifier**, **BareTypeLeaf**,
//!   **SigiledTypeExpr**, **LiteralPassThrough** → [`single_poll`] handlers
//! - **NonCallableHead** (a literal/empty/lazy head) → a direct
//!   `DispatchFailed` raise carrying the offending head
//!
//! State and transitions live with their shape; this file keeps the cross-shape glue. Every
//! per-shape handler *decides* against a read-only [`SchedulerView`] and returns a
//! [`Outcome`] the [`harness`] applies — the router and harness hold the only
//! `&mut Scheduler`, so the shape modules never mutate the scheduler (nor spell its field names).

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{Carried, Parseable};
use crate::machine::{KError, KErrorKind, NodeId, Resolution, Scope, TraceFrame};

use super::nodes::{DispatchCombineFinish, NodeOutput, NodeStep, NodeWork};
use super::scheduler::Scheduler;
use crate::machine::core::kfunction::action::FramePlacement;

pub(in crate::machine::execute) mod apply_callable;
mod constructors;
mod ctx;
mod exec;
pub(in crate::machine) mod field_list;
pub(in crate::machine::execute) mod fn_value;
mod harness;
pub(in crate::machine::execute) mod head_deferred;
pub(in crate::machine::execute) mod keyworded;
pub(in crate::machine::execute) mod operator_chain;
pub(in crate::machine) mod resolve_dispatch;
pub(in crate::machine) mod resolve_type_expr;
pub(in crate::machine::execute) mod single_poll;

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use ctx::SchedulerView;
pub(in crate::machine::execute) use harness::run_dispatch_combine_finish;
pub(in crate::machine::execute) use super::outcome::{Continuation, DispatchDep, Outcome};
pub(crate) use field_list::defer_field_list_action;
use fn_value::FnValueState;
use keyworded::KeywordedState;
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_dispatch::{NameOutcome, ResolveOutcome, Resolved};
pub use resolve_type_expr::ResolveTypeExprOutcome;
pub(crate) use resolve_type_expr::{resolve_type_leaf_carrier, TypeLeafCarrier};
use single_poll::{BareTypeState, CtorState};

/// The shape classification and classifier live in
/// [`crate::machine::model::ast`] (pure-structural, cached on the node at parse
/// time); re-exported here so dispatch-internal call sites and tests keep the
/// `dispatch::{DispatchShape, classify_dispatch_shape}` path.
#[allow(unused_imports)]
pub use crate::machine::model::ast::{classify_dispatch_shape, DispatchShape};

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type)
/// against `scope`. `consumer = Some(idx)` enables the cycle check;
/// `consumer = None` skips it.
pub(super) fn resolve_name_part<'run>(
    scope: &Scope<'run>,
    part: &ExpressionPart<'run>,
    scheduler: &Scheduler<'run>,
    consumer: Option<NodeId>,
) -> NameOutcome<'run> {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (n.as_str(), None),
        ExpressionPart::Type(t) => (t.as_str(), Some(t)),
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = scheduler.chain_deref();
    match scope.resolve_with_chain(name, chain) {
        Resolution::Placeholder(producer) => {
            return disposition_for_producer(scheduler, name, producer, consumer);
        }
        Resolution::Value(obj) if is_type.is_none() => {
            return NameOutcome::Resolved(Carried::Object(obj));
        }
        Resolution::Value(_) | Resolution::UnboundName => {}
    }
    match is_type {
        // The bare-leaf type token routes through the memoized, park-capable bridge. A
        // not-yet-sealed referent parks on its single producer (a visible type alias has
        // already resolved its RHS, so a leaf parks on at most one binder), reusing the
        // same ready/cycle disposition the value-side placeholder arm applies.
        Some(t) => match resolve_type_leaf_carrier(scope, t, scheduler.active_chain_clone()) {
            TypeLeafCarrier::Resolved(kt) => NameOutcome::Resolved(Carried::Type(kt)),
            TypeLeafCarrier::Unbound(n) => NameOutcome::Unbound(n),
            TypeLeafCarrier::Park(producers) => match producers.first() {
                Some(producer) => disposition_for_producer(scheduler, name, *producer, consumer),
                None => NameOutcome::Unbound(name.to_string()),
            },
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Map a still-finalizing producer for a parked name onto a [`NameOutcome`]: a
/// ready-but-errored producer surfaces its error, a ready-and-bound producer means the
/// name finalized to a non-shadowing value (`Unbound`), a parking edge that would close a
/// wake cycle is `Cycle`, and otherwise the name parks on the producer.
fn disposition_for_producer<'run>(
    scheduler: &Scheduler<'run>,
    name: &str,
    producer: NodeId,
    consumer: Option<NodeId>,
) -> NameOutcome<'run> {
    if scheduler.is_result_ready(producer) {
        match scheduler.read_result(producer) {
            Err(e) => NameOutcome::ProducerErrored(e.clone_for_propagation()),
            Ok(_) => NameOutcome::Unbound(name.to_string()),
        }
    } else if matches!(consumer, Some(c) if scheduler.would_create_cycle(producer, c)) {
        NameOutcome::Cycle(name.to_string())
    } else {
        NameOutcome::Parked(producer)
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart`,
/// used to render the `cycle in type alias <name>` deadlock sample.
pub(super) fn bare_name_of<'run>(part: &ExpressionPart<'run>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}

/// One staged submission queued by the keyworded part walk.
pub(in crate::machine::execute) enum PendingSub<'run> {
    Reuse(NodeId),
    Dispatch(KExpression<'run>),
    ListLit(Vec<ExpressionPart<'run>>),
    DictLit(Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>),
    RecordLit(Vec<(String, ExpressionPart<'run>)>),
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'run> {
    pub new_parts: Vec<Spanned<ExpressionPart<'run>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'run>)>,
}

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// - `Named` — a `{x = 1}` record literal: the sole named-argument surface (function and
///   functor calls, struct construction).
/// - `Positional` — a `(err "x")` paren group: positional construction (tagged unions,
///   newtypes). The verb-carrier decides which shape it admits; the mismatched shape
///   surfaces a loud `DispatchFailed`.
pub(super) enum CallBody<'run> {
    Named(Vec<(String, ExpressionPart<'run>)>),
    Positional(Vec<Spanned<ExpressionPart<'run>>>),
}

/// Classify the single body part of a `head (...)` / `head {...}` call from
/// `expr.parts[1..]`. The body must be exactly one nested-parens (`Positional`) or one
/// record literal (`Named`); anything else is a non-match.
pub(super) fn extract_call_body<'run>(expr: &KExpression<'run>) -> Result<CallBody<'run>, KError> {
    match expr.parts[1..].as_ref() {
        [Spanned {
            value: ExpressionPart::RecordLiteral(fields),
            ..
        }] => Ok(CallBody::Named(fields.clone())),
        [Spanned {
            value: ExpressionPart::Expression(inner),
            ..
        }] => Ok(CallBody::Positional(inner.parts.clone())),
        _ => Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        })),
    }
}

/// Reason strings for the loud `DispatchFailed` raised when a call body's surface shape
/// doesn't match what the resolved verb-carrier admits.
pub(super) const NAMED_ONLY: &str =
    "named arguments use a record literal `{name = value}`, not a parenthesized group";
pub(super) const POSITIONAL_ONLY: &str =
    "positional construction takes `(value)`, not a record literal `{name = value}`";

/// Loud non-match for a call body whose surface shape the resolved carrier doesn't admit.
pub(super) fn body_shape_err<'run>(expr: &KExpression<'run>, reason: &str) -> Outcome<'run> {
    Outcome::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
        expr: expr.summarize(),
        reason: reason.to_string(),
    })))
}

/// Clone a dep's terminal error and attach a caller-chosen frame.
/// `frame = None` is the frameless variant.
pub(super) fn propagate_dep_error(e: &KError, frame: Option<TraceFrame>) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f),
        None => cloned,
    }
}

/// Shape a dep-error terminal with the `<bind>` surface frame keyed
/// off `working_expr`.
pub(super) fn bind_frame_err<'run>(
    e: &KError,
    working_expr: &KExpression<'run>,
) -> Outcome<'run> {
    let frame = TraceFrame::from_expr("<bind>", working_expr);
    Outcome::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))))
}

// ---------- Outcome constructors (the dispatch-currency → Outcome mapping) ----------

/// Park the slot on `deps` as a [`NodeWork::DispatchCombine`](super::nodes::NodeWork) whose
/// `finish` runs over their resolved values (the dispatch Combine — short-circuits on dep error).
/// Every dep is owned (`park_count: 0`); `free` reclaims `Reuse` producers consumed inline.
pub(in crate::machine::execute) fn park_combine<'run>(
    deps: Vec<DispatchDep<'run>>,
    dep_error_frame: Option<TraceFrame>,
    finish: DispatchCombineFinish<'run>,
    free: Vec<usize>,
) -> Outcome<'run> {
    Outcome::ParkThenContinue {
        deps,
        park_count: 0,
        cont: Continuation::Finish(finish),
        dep_error_frame,
        free,
    }
}

/// Park the slot on `producers` (notify edges) and re-run its dispatch decide on wake (the
/// `ParkSelf` shape). The producers are the to-wait set the decide already filtered.
pub(in crate::machine::execute) fn park_self<'run>(
    producers: Vec<NodeId>,
    state: DispatchState<'run>,
) -> Outcome<'run> {
    Outcome::ParkThenContinue {
        park_count: producers.len(),
        deps: producers.into_iter().map(DispatchDep::Existing).collect(),
        cont: Continuation::Replay(state),
        dep_error_frame: None,
        free: Vec::new(),
    }
}

/// Park a bare-identifier slot on the single `producer` that binds its name, then *become* that
/// producer's resolved value (the push/notify single-producer `Lift` forward).
pub(in crate::machine::execute) fn park_lift<'run>(producer: NodeId) -> Outcome<'run> {
    Outcome::ParkThenContinue {
        deps: vec![DispatchDep::Existing(producer)],
        park_count: 1,
        cont: Continuation::Forward(producer),
        dep_error_frame: None,
        free: Vec::new(),
    }
}

/// Replace the slot with a fresh frameless `Dispatch` of `inner` — the decide reduced its
/// expression to a nested one to re-classify (`(inner)`, `:(...)` unwrap).
pub(in crate::machine::execute) fn become_dispatch<'run>(inner: KExpression<'run>) -> Outcome<'run> {
    Outcome::Continue {
        work: NodeWork::dispatch(inner),
        frame: FramePlacement::Inherit,
        contract: None,
        block_entry: None,
        body_index: 0,
    }
}

/// Walk raw parts emitting an `Identifier("")` placeholder at every
/// eager slot and a parallel staged-subs Vec; non-eager parts pass
/// through unchanged.
///
/// `wrap_indices` names bare-name value slots (the `wrap_indices` set from
/// [`KFunction::classify_for_pick`](crate::machine::core::kfunction::KFunction::classify_for_pick))
/// to resolve before bind. The keyword path resolves these via `bare_outcomes`
/// because it must know their carried type *during* overload selection; the
/// post-pick named-argument / function-value tail has already committed to one
/// callable, so it resolves them by sub-Dispatch through the same eager-subs
/// parking/resume path as `Expression` parts. Callers with no committed pick
/// (the keyworded `Deferred` arm, which re-resolves on finish) pass `&[]`.
pub(super) fn stage_all_eager_parts<'run>(
    parts: Vec<Spanned<ExpressionPart<'run>>>,
    wrap_indices: &[usize],
) -> (
    Vec<Spanned<ExpressionPart<'run>>>,
    Vec<(usize, PendingSub<'run>)>,
) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'run>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, PendingSub<'run>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if wrap_indices.contains(&i) {
            // Bare-name value slot: resolve the name through a single-part
            // sub-Dispatch (the `BareIdentifier` / `BareTypeLeaf` fast lane), so
            // the resolved `Future` carrier reaches `accepts_part` at bind.
            let wrapped = KExpression::new(vec![Spanned {
                value: part.value,
                span,
            }]);
            staged.push((i, PendingSub::Dispatch(wrapped)));
            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            continue;
        }
        match part.value {
            ExpressionPart::Expression(boxed) => {
                staged.push((i, PendingSub::Dispatch(*boxed)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::SigiledTypeExpr(boxed) => {
                let wrapped =
                    KExpression::new(vec![Spanned::bare(ExpressionPart::SigiledTypeExpr(boxed))]);
                staged.push((i, PendingSub::Dispatch(wrapped)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::RecordType(boxed) => {
                let wrapped =
                    KExpression::new(vec![Spanned::bare(ExpressionPart::RecordType(boxed))]);
                staged.push((i, PendingSub::Dispatch(wrapped)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::ListLiteral(items) => {
                staged.push((i, PendingSub::ListLit(items)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::DictLiteral(pairs) => {
                staged.push((i, PendingSub::DictLit(pairs)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::RecordLiteral(fields) => {
                staged.push((i, PendingSub::RecordLit(fields)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            other => new_parts.push(Spanned { value: other, span }),
        }
    }
    (new_parts, staged)
}

// ---------- State carrier ----------

/// Universal birth state of a Dispatch slot — the shape before
/// classification. Embedded by value in every per-variant state
/// struct so `pre_subs` rides along structurally rather than by
/// convention.
pub(in crate::machine::execute) struct Initialized {
    /// Pre-submitted sub-Dispatches keyed by their slot index in
    /// `expr.parts`; populated by submit-time recursion for
    /// binder-shaped expressions, empty otherwise.
    pub(in crate::machine::execute) pre_subs: Vec<(usize, NodeId)>,
}

/// One variant per [`DispatchShape`], plus the pre-classification
/// `Initialized` birth state. `Keyworded` and `FunctionValueCall` are
/// boxed because each carries multiple independent `Option<Track>`
/// fields; inlining would push every `DispatchState`-carrying type
/// past clippy's `large_enum_variant` threshold.
pub(in crate::machine::execute) enum DispatchState<'run> {
    Initialized(Initialized),
    BareTypeLeaf(BareTypeState<'run>),
    /// Boxed for the same reason as `Keyworded` / `FunctionValueCall`: the
    /// `CtorState` carries an eager-subs `CtorTrack` (schemas, staged values) or a
    /// head-placeholder `KExpression`, either of which would push the by-value
    /// `DispatchState` past clippy's `large_enum_variant` threshold.
    TypeCall(Box<CtorState<'run>>),
    FunctionValueCall(Box<FnValueState<'run>>),
    Keyworded(Box<KeywordedState<'run>>),
}

impl<'run> DispatchState<'run> {
    /// Construct the universal birth state. Every submission and
    /// re-park site goes through this constructor so `pre_subs` is the
    /// only field any caller names.
    pub(in crate::machine::execute) fn initialized(pre_subs: Vec<(usize, NodeId)>) -> Self {
        DispatchState::Initialized(Initialized { pre_subs })
    }

    /// Expression carried by the state itself for parked `Keyworded`
    /// or `FunctionValueCall` slots. Track installers drop
    /// `NodeWork::Dispatch.expr` to an empty placeholder on park, so
    /// the drain-end deadlock summary needs this fallback to render a
    /// parked sample.
    pub(in crate::machine::execute) fn parked_carrier_expr(&self) -> Option<&KExpression<'run>> {
        match self {
            DispatchState::Keyworded(ks) => ks.track.as_ref().map(|t| t.carrier_expr()),
            DispatchState::FunctionValueCall(fs) => Some(&fs.head_placeholder.expr),
            _ => None,
        }
    }
}

// ---------- Cross-shape driver ----------

/// Stateful dispatch driver. Classifies the slot's shape and routes to
/// the matching per-shape entry. Fast-lane variants terminalize (or
/// single-producer-park) in one poll; only `Keyworded` and
/// `FunctionValueCall` carry tracks that can re-enter via the resume
/// arms.
pub(in crate::machine::execute) fn run_dispatch<'run>(
    sched: &mut Scheduler<'run>,
    expr: KExpression<'run>,
    state: DispatchState<'run>,
    idx: usize,
) -> NodeStep<'run> {
    let _wakes = sched.take_recent_wakes(NodeId(idx));
    let init = match state {
        DispatchState::Initialized(i) => i,
        // Each parked-state resume decides against a read-only view; the router clears the
        // resuming slot's stale dep edges (where the resume depends on it) before deciding,
        // then applies the returned outcome — the resume itself issues no graph write.
        DispatchState::Keyworded(ks) => {
            let outcome = ks.resume(&SchedulerView::new(sched), idx);
            return sched.apply_outcome(outcome, idx);
        }
        DispatchState::FunctionValueCall(fs) => {
            let outcome = fs.resume(&SchedulerView::new(sched));
            return sched.apply_outcome(outcome, idx);
        }
        DispatchState::TypeCall(cs) => {
            sched.clear_dep_edges(idx);
            let outcome = (*cs).resume(&SchedulerView::new(sched));
            return sched.apply_outcome(outcome, idx);
        }
        DispatchState::BareTypeLeaf(bs) if bs.park.is_some() => {
            sched.clear_dep_edges(idx);
            let outcome = bs.resume(&SchedulerView::new(sched));
            return sched.apply_outcome(outcome, idx);
        }
        _ => unreachable!(
            "remaining fast-lane stateful variants terminalize in one poll; \
             only Keyworded, FunctionValueCall, TypeCall, and a parked BareTypeLeaf \
             re-enter from a parked track (HeadDeferred parks as a DispatchCombine, \
             resumed by the scheduler, not a Dispatch state)"
        ),
    };
    match expr.shape() {
        DispatchShape::BareTypeLeaf => {
            debug_assert!(init.pre_subs.is_empty());
            let t = match &expr.parts[0].value {
                ExpressionPart::Type(t) => t.clone(),
                _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
            };
            let outcome = single_poll::bare_type_leaf(&SchedulerView::new(sched), &t);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::BareIdentifier => {
            debug_assert!(init.pre_subs.is_empty());
            let name = match &expr.parts[0].value {
                ExpressionPart::Identifier(n) => n.clone(),
                _ => unreachable!("BareIdentifier shape implies single Identifier part"),
            };
            let outcome = single_poll::bare_identifier(&SchedulerView::new(sched), name);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::FunctionValueCall => {
            debug_assert!(init.pre_subs.is_empty());
            let _ = init;
            let outcome = FnValueState::initial(&SchedulerView::new(sched), expr);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::TypeCall => {
            debug_assert!(init.pre_subs.is_empty());
            let outcome = single_poll::type_call(&SchedulerView::new(sched), expr);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::HeadDeferred => {
            debug_assert!(init.pre_subs.is_empty());
            sched.apply_outcome(head_deferred::initial_expr(expr), idx)
        }
        DispatchShape::TypeHeadDeferred => {
            debug_assert!(init.pre_subs.is_empty());
            sched.apply_outcome(head_deferred::initial_type(expr), idx)
        }
        // Slot-terminal (TRY-catchable), uniform with every other dispatch failure —
        // a non-callable head is a runtime error, not a fatal `execute()` abort.
        DispatchShape::NonCallableHead => {
            NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: format!(
                    "head is not callable: `{}`",
                    expr.parts
                        .first()
                        .map(|p| p.value.summarize())
                        .unwrap_or_else(|| "<empty>".into())
                ),
            })))
        }
        DispatchShape::OperatorChain => {
            debug_assert!(init.pre_subs.is_empty());
            // Decide against a read-only view (immutable scheduler borrow), then reborrow
            // `&mut` through the harness to apply — the borrow contract the effect split rests
            // on. The `read_view` borrow ends at the `run` call (NLL), freeing `ctx` for apply.
            let outcome = operator_chain::run(&SchedulerView::new(sched), &expr);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::Keyworded => {
            let outcome = KeywordedState::initial(&SchedulerView::new(sched), expr, init.pre_subs, idx);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::SigiledTypeExpr => {
            debug_assert!(init.pre_subs.is_empty());
            sched.apply_outcome(single_poll::sigiled_type_expr(expr), idx)
        }
        DispatchShape::RecordType => {
            debug_assert!(init.pre_subs.is_empty());
            let outcome = single_poll::record_type(&SchedulerView::new(sched), expr);
            sched.apply_outcome(outcome, idx)
        }
        DispatchShape::LiteralPassThrough => {
            debug_assert!(init.pre_subs.is_empty());
            let outcome = single_poll::literal_pass_through(&SchedulerView::new(sched), expr);
            sched.apply_outcome(outcome, idx)
        }
    }
}

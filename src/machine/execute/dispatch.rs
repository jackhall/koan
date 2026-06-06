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
//! State and transitions live with their shape; this file keeps the
//! cross-shape glue. Every per-shape handler takes a
//! [`DispatchCtx`] — the typed facade over `&mut Scheduler<'a>` — so the
//! shape modules never spell scheduler field names.

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Parseable;
use crate::machine::{Frame, KError, KErrorKind, NodeId, Resolution, Scope};

use super::nodes::{NodeOutput, NodeStep};
use super::scheduler::Scheduler;

pub(in crate::machine::execute) mod apply_callable;
mod constructors;
mod ctx;
pub(in crate::machine::execute) mod fn_value;
pub(in crate::machine::execute) mod head_deferred;
pub(in crate::machine::execute) mod keyworded;
pub(in crate::machine::execute) mod operator_chain;
pub(in crate::machine) mod resolve_dispatch;
pub(in crate::machine) mod resolve_type_expr;
pub(in crate::machine::execute) mod single_poll;

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use ctx::DispatchCtx;
use fn_value::FnValueState;
use head_deferred::HeadDeferredState;
use keyworded::KeywordedState;
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_dispatch::{NameOutcome, ResolveOutcome, Resolved};
pub use resolve_type_expr::ResolveTypeExprOutcome;
pub(crate) use resolve_type_expr::{resolve_type_leaf_carrier, TypeLeafCarrier};
use single_poll::{BareIdState, BareTypeState, CtorState, LitState, SigilState};

/// The shape classification and classifier live in
/// [`crate::machine::model::ast`] (pure-structural, cached on the node at parse
/// time); re-exported here so dispatch-internal call sites and tests keep the
/// `dispatch::{DispatchShape, classify_dispatch_shape}` path.
#[allow(unused_imports)]
pub use crate::machine::model::ast::{classify_dispatch_shape, DispatchShape};

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type)
/// against `scope`. `consumer = Some(idx)` enables the cycle check;
/// `consumer = None` skips it.
pub(super) fn resolve_name_part<'a>(
    scope: &'a Scope<'a>,
    part: &ExpressionPart<'a>,
    scheduler: &Scheduler<'a>,
    consumer: Option<NodeId>,
) -> NameOutcome<'a> {
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
            return NameOutcome::Resolved(obj);
        }
        Resolution::Value(_) | Resolution::UnboundName => {}
    }
    match is_type {
        // The bare-leaf type token routes through the memoized, park-capable bridge. A
        // not-yet-sealed referent parks on its single producer (a visible type alias has
        // already resolved its RHS, so a leaf parks on at most one binder), reusing the
        // same ready/cycle disposition the value-side placeholder arm applies.
        Some(t) => match resolve_type_leaf_carrier(scope, t, scheduler.active_chain_clone()) {
            TypeLeafCarrier::Resolved(obj) => NameOutcome::Resolved(obj),
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
fn disposition_for_producer<'a>(
    scheduler: &Scheduler<'a>,
    name: &str,
    producer: NodeId,
    consumer: Option<NodeId>,
) -> NameOutcome<'a> {
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
pub(super) fn bare_name_of<'a>(part: &ExpressionPart<'a>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}

/// One staged submission queued by the keyworded part walk.
pub(in crate::machine::execute) enum PendingSub<'a> {
    Reuse(NodeId),
    Dispatch(KExpression<'a>),
    ListLit(Vec<ExpressionPart<'a>>),
    DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    RecordLit(Vec<(String, ExpressionPart<'a>)>),
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'a> {
    pub new_parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'a>)>,
}

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// - `Named` — a `{x = 1}` record literal: the sole named-argument surface (function and
///   functor calls, struct construction).
/// - `Positional` — a `(err "x")` paren group: positional construction (tagged unions,
///   newtypes). The verb-carrier decides which shape it admits; the mismatched shape
///   surfaces a loud `DispatchFailed`.
pub(super) enum CallBody<'a> {
    Named(Vec<(String, ExpressionPart<'a>)>),
    Positional(Vec<Spanned<ExpressionPart<'a>>>),
}

/// Classify the single body part of a `head (...)` / `head {...}` call from
/// `expr.parts[1..]`. The body must be exactly one nested-parens (`Positional`) or one
/// record literal (`Named`); anything else is a non-match.
pub(super) fn extract_call_body<'a>(expr: &KExpression<'a>) -> Result<CallBody<'a>, KError> {
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
pub(super) fn body_shape_err<'a>(expr: &KExpression<'a>, reason: &str) -> NodeStep<'a> {
    NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
        expr: expr.summarize(),
        reason: reason.to_string(),
    })))
}

/// Clone a dep's terminal error and attach a caller-chosen frame.
/// `frame = None` is the frameless variant.
pub(super) fn propagate_dep_error(e: &KError, frame: Option<Frame>) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f),
        None => cloned,
    }
}

/// Shape a dep-error terminal with the `<bind>` surface frame keyed
/// off `working_expr`.
pub(super) fn bind_frame_err<'a>(e: &KError, working_expr: &KExpression<'a>) -> NodeStep<'a> {
    let frame = Frame::from_expr("<bind>", working_expr);
    NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))))
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
pub(super) fn stage_all_eager_parts<'a>(
    parts: Vec<Spanned<ExpressionPart<'a>>>,
    wrap_indices: &[usize],
) -> (
    Vec<Spanned<ExpressionPart<'a>>>,
    Vec<(usize, PendingSub<'a>)>,
) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, PendingSub<'a>)> = Vec::new();
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

/// Outcome of [`DispatchCtx::install_eager_subs`].
pub(in crate::machine::execute::dispatch) enum EagerSubsInstall<'a> {
    AllInline(KExpression<'a>),
    Parked(EagerSubsTrack<'a>),
    DepError(NodeStep<'a>),
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

/// Track state for the eager-subs sub-Dispatches a Keyworded or
/// FunctionValueCall slot is parked on. Each `(part_idx, sub_id)` is
/// the slot index in `working_expr.parts` to splice into at track
/// completion plus the sub NodeId (the Owned dep this slot installed
/// at park-install time).
pub(in crate::machine::execute) struct EagerSubsTrack<'a> {
    pub(in crate::machine::execute) working_expr: KExpression<'a>,
    pub(in crate::machine::execute) subs: Vec<(usize, NodeId)>,
    /// `Some(f)` is the FunctionValueCall install; resume binds `f`
    /// directly. `None` is the Keyworded install; resume re-runs
    /// `resolve_dispatch` — re-resolve is authoritative so
    /// an element-typed `Future(_)` revealed by an eager sub surfaces
    /// as `DispatchFailed` (non-match) rather than a bind-time
    /// `TypeMismatch`.
    pub(in crate::machine::execute) picked: Option<&'a KFunction<'a>>,
}

impl<'a> EagerSubsTrack<'a> {
    pub(in crate::machine::execute) fn keyworded(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    ) -> Self {
        Self {
            working_expr,
            subs,
            picked: None,
        }
    }

    pub(in crate::machine::execute) fn fn_value(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        picked: &'a KFunction<'a>,
    ) -> Self {
        Self {
            working_expr,
            subs,
            picked: Some(picked),
        }
    }
}

/// One variant per [`DispatchShape`], plus the pre-classification
/// `Initialized` birth state. `Keyworded` and `FunctionValueCall` are
/// boxed because each carries multiple independent `Option<Track>`
/// fields; inlining would push every `DispatchState`-carrying type
/// past clippy's `large_enum_variant` threshold.
pub(in crate::machine::execute) enum DispatchState<'a> {
    Initialized(Initialized),
    BareIdentifier(BareIdState<'a>),
    BareTypeLeaf(BareTypeState<'a>),
    /// Boxed for the same reason as `Keyworded` / `FunctionValueCall`: the
    /// `CtorState` carries an eager-subs `CtorTrack` (schemas, staged values) or a
    /// head-placeholder `KExpression`, either of which would push the by-value
    /// `DispatchState` past clippy's `large_enum_variant` threshold.
    TypeCall(Box<CtorState<'a>>),
    FunctionValueCall(Box<FnValueState<'a>>),
    /// Shared by the `HeadDeferred` and `TypeHeadDeferred` shapes; the state's
    /// `type_only` flag selects the admitted-arm set on resume.
    HeadDeferred(Box<HeadDeferredState<'a>>),
    LiteralPassThrough(LitState<'a>),
    SigiledTypeExpr(SigilState<'a>),
    Keyworded(Box<KeywordedState<'a>>),
}

impl<'a> DispatchState<'a> {
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
    pub(in crate::machine::execute) fn parked_carrier_expr(&self) -> Option<&KExpression<'a>> {
        match self {
            DispatchState::Keyworded(ks) => ks.track.as_ref().map(|t| t.carrier_expr()),
            DispatchState::FunctionValueCall(fs) => {
                if let Some(track) = &fs.eager_subs {
                    return Some(&track.working_expr);
                }
                if let Some(track) = &fs.head_placeholder {
                    return Some(&track.expr);
                }
                None
            }
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
pub(in crate::machine::execute) fn run_dispatch<'a>(
    ctx: &mut DispatchCtx<'a, '_>,
    expr: KExpression<'a>,
    state: DispatchState<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> Result<NodeStep<'a>, KError> {
    let _wakes = ctx.take_recent_wakes(NodeId(idx));
    let init = match state {
        DispatchState::Initialized(i) => i,
        DispatchState::Keyworded(ks) => return ks.resume(ctx, scope, idx),
        DispatchState::FunctionValueCall(fs) => return fs.resume(ctx, scope, idx),
        DispatchState::TypeCall(cs) => return (*cs).resume(ctx, scope, idx),
        DispatchState::HeadDeferred(hd) => return Ok(hd.resume(ctx, scope, idx)),
        DispatchState::BareTypeLeaf(bs) if bs.park.is_some() => return bs.resume(ctx, scope, idx),
        _ => unreachable!(
            "remaining fast-lane stateful variants terminalize in one poll; \
             only Keyworded, FunctionValueCall, TypeCall, HeadDeferred, and a \
             parked BareTypeLeaf re-enter from a parked track"
        ),
    };
    match expr.shape() {
        DispatchShape::BareTypeLeaf => {
            debug_assert!(init.pre_subs.is_empty());
            let t = match &expr.parts[0].value {
                ExpressionPart::Type(t) => t.clone(),
                _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
            };
            Ok(single_poll::bare_type_leaf(ctx, &t, scope, idx))
        }
        DispatchShape::BareIdentifier => {
            debug_assert!(init.pre_subs.is_empty());
            let name = match &expr.parts[0].value {
                ExpressionPart::Identifier(n) => n.clone(),
                _ => unreachable!("BareIdentifier shape implies single Identifier part"),
            };
            Ok(single_poll::bare_identifier(ctx, name, scope, idx))
        }
        DispatchShape::FunctionValueCall => {
            debug_assert!(init.pre_subs.is_empty());
            let _ = init;
            FnValueState::initial(ctx, expr, scope, idx)
        }
        DispatchShape::TypeCall => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(single_poll::type_call(ctx, expr, scope, idx))
        }
        DispatchShape::HeadDeferred => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(HeadDeferredState::initial_expr(ctx, expr, scope, idx))
        }
        DispatchShape::TypeHeadDeferred => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(HeadDeferredState::initial_type(ctx, expr, scope, idx))
        }
        DispatchShape::NonCallableHead => Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: format!(
                "head is not callable: `{}`",
                expr.parts
                    .first()
                    .map(|p| p.value.summarize())
                    .unwrap_or_else(|| "<empty>".into())
            ),
        })),
        DispatchShape::OperatorChain => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(operator_chain::run(ctx, &expr, scope))
        }
        DispatchShape::Keyworded => KeywordedState::initial(ctx, expr, init.pre_subs, scope, idx),
        DispatchShape::SigiledTypeExpr => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(single_poll::sigiled_type_expr(expr))
        }
        DispatchShape::LiteralPassThrough => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(single_poll::literal_pass_through(ctx, expr, scope, idx))
        }
    }
}

//! Dispatch shape router, classifier, and shared spine.
//!
//! [`run_dispatch`] classifies the slot via [`classify_dispatch_shape`]
//! and routes to one of the five shape handlers:
//!
//! - **Keyworded** (any keyword present, or a head that isn't a
//!   fast-lane shape) → [`keyworded::KeywordedState`]
//! - **FunctionValueCall** (lowercase Identifier head + nested-parens
//!   body) → [`fn_value::FnValueState`]
//! - **BareIdentifier**, **BareTypeLeaf**, **ConstructorCall**,
//!   **SigiledTypeExpr** → [`single_poll`] handlers
//!
//! State and transitions live with their shape; this file keeps the
//! cross-shape glue. Every per-shape handler takes a
//! [`DispatchCtx`] — the typed facade over `&mut Scheduler<'a>` — so the
//! shape modules never spell scheduler field names.

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeParams};
use crate::machine::model::Parseable;
use crate::machine::{
    Frame, KError, KErrorKind, NodeId, Resolution, Scope,
};

use super::scheduler::Scheduler;
use super::nodes::{NodeOutput, NodeStep};

mod ctx;
pub(in crate::machine::execute) mod fn_value;
pub(in crate::machine::execute) mod keyworded;
pub(in crate::machine) mod resolve_dispatch;
pub(in crate::machine) mod resolve_type_expr;
pub(in crate::machine::execute) mod single_poll;

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use ctx::DispatchCtx;
use fn_value::FnValueState;
use keyworded::KeywordedState;
pub use resolve_dispatch::{NameOutcome, ResolveOutcome, Resolved};
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_type_expr::{coerce_type_token_value, ResolveTypeExprOutcome};
use single_poll::{BareIdState, BareTypeState, CtorState, LitState, SigilState};

/// Pre-walk classification of a `KExpression` into the no-keyword
/// fast-lane shapes plus the catch-all keyword-bearing shape.
pub(super) enum DispatchShape {
    BareIdentifier,
    BareTypeLeaf,
    /// Type-constructor call: head is a leaf `Type` and `parts[1..]`
    /// is non-empty.
    ConstructorCall,
    /// Function-value call: head is a lowercase `Identifier`,
    /// followed by ≥1 non-keyword parts.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper.
    SigiledTypeExpr,
    /// Single-part literal-shaped expression — `Literal`, `Future`,
    /// nested `Expression`, `ListLiteral`, or `DictLiteral`. Surfaces
    /// the inner value without a bucket lookup.
    LiteralPassThrough,
    /// A keyword appears anywhere in `expr.parts`, OR the expression
    /// doesn't fit any fast-lane shape.
    Keyworded,
}

/// Sweeps every part for `Keyword` first so a mixed shape like
/// `(f IF x)` goes to `Keyworded`; only with the no-keyword
/// precondition established do we branch on head shape.
pub(super) fn classify_dispatch_shape(expr: &KExpression<'_>) -> DispatchShape {
    if expr.parts.iter().any(|p| matches!(&p.value, ExpressionPart::Keyword(_))) {
        return DispatchShape::Keyworded;
    }
    if let [only] = expr.parts.as_slice() {
        return match &only.value {
            ExpressionPart::Identifier(_) => DispatchShape::BareIdentifier,
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                DispatchShape::BareTypeLeaf
            }
            ExpressionPart::SigiledTypeExpr(_) => DispatchShape::SigiledTypeExpr,
            ExpressionPart::Literal(_)
            | ExpressionPart::Future(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_) => DispatchShape::LiteralPassThrough,
            _ => DispatchShape::Keyworded,
        };
    }
    let Some(head_part) = expr.parts.first() else {
        return DispatchShape::Keyworded;
    };
    match &head_part.value {
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
            DispatchShape::ConstructorCall
        }
        ExpressionPart::Identifier(_) => DispatchShape::FunctionValueCall,
        _ => DispatchShape::Keyworded,
    }
}

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
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
            (t.name.as_str(), Some(t))
        }
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = scheduler.chain_deref();
    match scope.resolve_with_chain(name, chain) {
        Resolution::Placeholder(producer) => {
            return if scheduler.is_result_ready(producer) {
                match scheduler.read_result(producer) {
                    Err(e) => NameOutcome::ProducerErrored(e.clone_for_propagation()),
                    Ok(_) => NameOutcome::Unbound(name.to_string()),
                }
            } else if matches!(consumer, Some(c) if scheduler.would_create_cycle(producer, c))
            {
                NameOutcome::Cycle(name.to_string())
            } else {
                NameOutcome::Parked(producer)
            };
        }
        Resolution::Value(obj) if is_type.is_none() => {
            return NameOutcome::Resolved(obj);
        }
        Resolution::Value(_) | Resolution::UnboundName => {}
    }
    match is_type {
        Some(t) => match coerce_type_token_value(scope, t, chain) {
            Ok(obj) => NameOutcome::Resolved(obj),
            Err(KError { kind: KErrorKind::UnboundName(n), .. }) => NameOutcome::Unbound(n),
            Err(e) => NameOutcome::ProducerErrored(e),
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart`,
/// used to render the `cycle in type alias <name>` deadlock sample.
pub(super) fn bare_name_of<'a>(part: &ExpressionPart<'a>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => Some(t.name.clone()),
        _ => None,
    }
}

/// One staged submission queued by the keyworded part walk.
pub(in crate::machine::execute) enum PendingSub<'a> {
    Reuse(NodeId),
    Dispatch(KExpression<'a>),
    ListLit(Vec<ExpressionPart<'a>>),
    DictLit(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'a> {
    pub new_parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'a>)>,
}

/// Pull the inner parts of a `f (...)` call out of `expr.parts[1..]`.
/// `FunctionValueCall` only guarantees ≥1 non-keyword body part; the
/// body must be exactly one nested-parens or this is a non-match.
pub(super) fn extract_named_call_inner<'a>(
    expr: &KExpression<'a>,
) -> Result<Vec<Spanned<ExpressionPart<'a>>>, KError> {
    let [Spanned { value: ExpressionPart::Expression(inner), .. }] = expr.parts[1..].as_ref()
    else {
        return Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        }));
    };
    Ok(inner.parts.clone())
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
pub(super) fn stage_all_eager_parts<'a>(
    parts: Vec<Spanned<ExpressionPart<'a>>>,
) -> (Vec<Spanned<ExpressionPart<'a>>>, Vec<(usize, PendingSub<'a>)>) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, PendingSub<'a>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        match part.value {
            ExpressionPart::Expression(boxed) => {
                staged.push((i, PendingSub::Dispatch(*boxed)));
                new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            }
            ExpressionPart::SigiledTypeExpr(boxed) => {
                let wrapped = KExpression::new(vec![Spanned::bare(
                    ExpressionPart::SigiledTypeExpr(boxed),
                )]);
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
        Self { working_expr, subs, picked: None }
    }

    pub(in crate::machine::execute) fn fn_value(
        working_expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        picked: &'a KFunction<'a>,
    ) -> Self {
        Self { working_expr, subs, picked: Some(picked) }
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
    ConstructorCall(CtorState<'a>),
    FunctionValueCall(Box<FnValueState<'a>>),
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
    pub(in crate::machine::execute) fn parked_carrier_expr(
        &self,
    ) -> Option<&KExpression<'a>> {
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
        _ => unreachable!(
            "remaining fast-lane stateful variants terminalize in one poll; \
             only Keyworded and FunctionValueCall re-enter from a parked track"
        ),
    };
    match classify_dispatch_shape(&expr) {
        DispatchShape::BareTypeLeaf => {
            debug_assert!(init.pre_subs.is_empty());
            let t = match &expr.parts[0].value {
                ExpressionPart::Type(t) => t.clone(),
                _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
            };
            Ok(single_poll::bare_type_leaf(ctx, &t, scope))
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
        DispatchShape::ConstructorCall => {
            debug_assert!(init.pre_subs.is_empty());
            Ok(single_poll::constructor_call(ctx, expr, scope, idx))
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

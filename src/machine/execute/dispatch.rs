//! Dispatch shape router, classifier, and shared spine.
//!
//! [`classify_dispatch`] classifies the slot via [`classify_dispatch_shape`]
//! and routes by shape:
//!
//! - **Keyworded** (a keyword is present) → [`keyworded::initial`]
//! - **FunctionValueCall** (lowercase Identifier head) →
//!   [`fn_value::initial`]
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
//! [`Outcome`] that [`KoanRuntime`](super::runtime::KoanRuntime) applies — the harness holds the
//! only `&mut Scheduler`, so the shape modules never mutate the scheduler (nor spell its field
//! names).

use crate::machine::model::Carried;
use crate::machine::model::TypeResolution;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::{KError, KErrorKind, NameLookup, NodeId, Scope, TraceFrame};
use crate::source::Spanned;

use super::ignore_results;
use super::nodes::{ChainOp, NodeWork};
use super::obligation::{with_obligation, ReturnObligation};
use super::runtime::KoanWorkload;
use crate::machine::core::{BlockEntry, FramePlacement};
use crate::scheduler::{Deps, ProducerDisposition, ResolvedDeps, Scheduler};

// The dep currency lives in core (`action.rs`) so an `Action` can carry it; re-exported here as the
// dispatch-side view `Outcome` consumers reach through `super::dispatch`.
pub(in crate::machine::execute) use crate::machine::core::{
    BodyPlacement, DepPlacement, DepRequest,
};

pub(in crate::machine::execute) mod apply_callable;
mod constructors;
mod ctx;
mod exec;
pub(in crate::machine) mod field_list;
pub(in crate::machine::execute) mod fn_value;
pub(in crate::machine::execute) mod head_deferred;
pub(in crate::machine::execute) mod keyworded;
mod literal;
pub(in crate::machine::execute) mod operator_chain;
pub(in crate::machine) mod resolve_dispatch;
pub(in crate::machine) mod resolve_type_identifier;
pub(in crate::machine::execute) mod single_poll;
mod submit;

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use super::outcome::{Await, Continuation, Outcome};
pub(crate) use constructors::{build_type_operand, seal_type_identity};
pub(in crate::machine::execute) use ctx::{with_node_scope, SchedulerView};
pub(crate) use field_list::{
    defer_field_list_action, defer_field_list_action_composed, BrandCompose,
};
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_dispatch::{DispatchOutcome, NameOutcome, Resolved};

/// The shape classification and classifier live in
/// [`crate::machine::model::ast`] (pure-structural, cached on the node at parse
/// time); re-exported here so dispatch-internal call sites and tests keep the
/// `dispatch::{DispatchShape, classify_dispatch_shape}` path.
#[allow(unused_imports)]
pub(crate) use crate::machine::model::{classify_dispatch_shape, DispatchShape};

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type)
/// against `scope`. `consumer = Some(idx)` enables the cycle check;
/// `consumer = None` skips it.
pub(super) fn resolve_name_part<'step>(
    scope: &Scope<'step>,
    part: &ExpressionPart<'step>,
    scheduler: &Scheduler<KoanWorkload>,
    active_chain: Option<&std::rc::Rc<crate::machine::LexicalFrame>>,
    consumer: Option<NodeId>,
    types: &crate::machine::model::TypeRegistry,
) -> NameOutcome<'step> {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (n.as_str(), None),
        ExpressionPart::Type(t) => (t.as_str(), Some(t)),
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = active_chain.map(|c| &**c);
    match scope.resolve_with_chain(name, chain) {
        Some(NameLookup::Parked(producer)) => {
            return disposition_for_producer(scheduler, name, producer, consumer);
        }
        // An Identifier part reads the value channel; a Type part takes the type ladder below.
        Some(NameLookup::Bound(obj)) if is_type.is_none() => {
            return NameOutcome::Resolved(Carried::Object(obj));
        }
        Some(NameLookup::Bound(_)) | None => {}
    }
    match is_type {
        // The bare-leaf type token routes through the memoized, park-capable bridge. A
        // not-yet-sealed referent parks on its single producer (a visible type alias has
        // already resolved its RHS, so a leaf parks on at most one binder), reusing the
        // same ready/cycle disposition the value-side placeholder arm applies.
        Some(t) => match scope.resolve_type_identifier(t, active_chain.cloned(), types) {
            TypeResolution::Done(kt) => NameOutcome::Resolved(Carried::Type(kt)),
            TypeResolution::Unbound(n) => NameOutcome::Unbound(n),
            TypeResolution::Park(producers) => match producers.first() {
                Some(producer) => disposition_for_producer(scheduler, name, *producer, consumer),
                None => NameOutcome::Unbound(name.to_string()),
            },
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Map a still-finalizing producer for a parked name onto a [`NameOutcome`]. A `Ready`
/// producer means the name finalized to a non-shadowing value, hence `Unbound`.
fn disposition_for_producer<'step>(
    scheduler: &Scheduler<KoanWorkload>,
    name: &str,
    producer: NodeId,
    consumer: Option<NodeId>,
) -> NameOutcome<'step> {
    match scheduler.producer_disposition(producer, consumer) {
        ProducerDisposition::Errored(e) => NameOutcome::ProducerErrored(e.clone_for_propagation()),
        ProducerDisposition::Ready => NameOutcome::Unbound(name.to_string()),
        ProducerDisposition::Cycle => NameOutcome::Cycle(name.to_string()),
        ProducerDisposition::Park => NameOutcome::Parked(producer),
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart`,
/// used to render the `cycle in type alias <name>` deadlock sample.
pub(super) fn bare_name_of<'step>(part: &ExpressionPart<'step>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}

/// The staged form of one eager part shape. Private plumbing: exists so the
/// six-shape set is written exactly once (in [`eager_shape`]) while staging
/// stays by-value. Adding a shape here forces a `stage_eager_part` arm via
/// match exhaustiveness.
enum EagerShape {
    /// `(...)` — the boxed inner expression dispatches directly.
    Subexpression,
    /// `:(…)` / `:{…}` — the whole part rewraps as a one-part sub-Dispatch
    /// to a type-side carrier.
    TypeExpression,
    ListLiteral,
    DictLiteral,
    RecordLiteral,
}

/// THE six-shape eager match — the only place the eager part-shape set is
/// enumerated. `None` means the part is not eager.
fn eager_shape(part: &ExpressionPart<'_>) -> Option<EagerShape> {
    match part {
        ExpressionPart::Expression(_) => Some(EagerShape::Subexpression),
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            Some(EagerShape::TypeExpression)
        }
        ExpressionPart::ListLiteral(_) => Some(EagerShape::ListLiteral),
        ExpressionPart::DictLiteral(_) => Some(EagerShape::DictLiteral),
        ExpressionPart::RecordLiteral(_) => Some(EagerShape::RecordLiteral),
        _ => None,
    }
}

/// True iff this part shape is one the eager loop schedules as a sub-Dispatch.
pub(in crate::machine::execute) fn is_eager_part(part: &ExpressionPart<'_>) -> bool {
    eager_shape(part).is_some()
}

/// Stage one eager part as the [`DepRequest`] the harness realizes; a non-eager
/// part round-trips back untouched. By-value — no clones on the staging path.
pub(in crate::machine::execute) fn stage_eager_part<'a>(
    part: ExpressionPart<'a>,
) -> Result<DepRequest<'a>, ExpressionPart<'a>> {
    match eager_shape(&part) {
        None => Err(part),
        Some(EagerShape::Subexpression) => {
            let ExpressionPart::Expression(boxed) = part else {
                unreachable!("eager_shape matched Subexpression")
            };
            Ok(DepRequest::Dispatch {
                expr: *boxed,
                placement: DepPlacement::OwnScope,
            })
        }
        Some(EagerShape::TypeExpression) => Ok(DepRequest::Dispatch {
            // Rewrap the whole part — the same shape `classify_aggregate_part`
            // builds, equivalent to the destructure-and-rewrap the walks did.
            expr: KExpression::new(vec![Spanned::bare(part)]),
            placement: DepPlacement::OwnScope,
        }),
        Some(EagerShape::ListLiteral) => {
            let ExpressionPart::ListLiteral(items) = part else {
                unreachable!("eager_shape matched ListLiteral")
            };
            Ok(DepRequest::ListLit(items))
        }
        Some(EagerShape::DictLiteral) => {
            let ExpressionPart::DictLiteral(pairs) = part else {
                unreachable!("eager_shape matched DictLiteral")
            };
            Ok(DepRequest::DictLit(pairs))
        }
        Some(EagerShape::RecordLiteral) => {
            let ExpressionPart::RecordLiteral(fields) = part else {
                unreachable!("eager_shape matched RecordLiteral")
            };
            Ok(DepRequest::RecordLit(fields))
        }
    }
}

/// The empty-`Identifier` hole a staged slot leaves in `new_parts`. Names the
/// existing placeholder convention; typing the sentinel as a real staged-slot
/// representation is a follow-up (see the roadmap item).
pub(in crate::machine::execute) fn staged_slot_placeholder<'a>() -> Spanned<ExpressionPart<'a>> {
    Spanned::bare(ExpressionPart::Identifier(String::new()))
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'step> {
    pub new_parts: Vec<Spanned<ExpressionPart<'step>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, DepRequest<'step>)>,
}

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// - `Named` — a `{x = 1}` record literal: the sole named-argument surface (function
///   calls, struct construction).
/// - `Positional` — a `(err "x")` paren group: positional construction (tagged unions,
///   newtypes). The verb-carrier decides which shape it admits; the mismatched shape
///   surfaces a loud `DispatchFailed`.
pub(super) enum CallBody<'step> {
    Named(Vec<(String, ExpressionPart<'step>)>),
    Positional(Vec<Spanned<ExpressionPart<'step>>>),
}

/// Classify the single body part of a `head (...)` / `head {...}` call from
/// `expr.parts[1..]`. The body must be exactly one nested-parens (`Positional`) or one
/// record literal (`Named`); anything else is a non-match.
pub(super) fn extract_call_body<'step>(
    expr: &KExpression<'step>,
) -> Result<CallBody<'step>, KError> {
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
pub(super) fn body_shape_err<'step>(expr: &KExpression<'step>, reason: &str) -> Outcome<'step> {
    Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
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

// ---------- Outcome constructors (the dispatch-currency → Outcome mapping) ----------

/// Park the slot on `producers` and re-run its `resume` decide on wake. `carrier` is the
/// parked expression's pre-rendered summary for the deadlock report (`None` when the park
/// carries no renderable form) — rendering it here keeps the AST out of the scheduler.
pub(in crate::machine::execute) fn park_resume<'step>(
    producers: Vec<NodeId>,
    carrier: Option<String>,
    resume: ResumeFn<'step>,
) -> Outcome<'step> {
    Outcome::ParkThenContinue {
        deps: Deps::from_parks(producers),
        continuation: Continuation::Resume { carrier, resume },
        dep_error_frame: None,
    }
}

/// A bare-identifier slot whose name binds to `producer`: the slot's result *is* `producer`'s
/// result, so the harness splices the slot out (no forwarding node) — see [`Outcome::Forward`].
pub(in crate::machine::execute) fn forward_to_producer<'step>(producer: NodeId) -> Outcome<'step> {
    Outcome::Forward(producer)
}

/// Replace the slot with a fresh frameless `Dispatch` of `inner` — the decide reduced its
/// expression to a nested one to re-classify (`(inner)`, `:(...)` unwrap). A re-classification that
/// carries an established tail-chain obligation wraps the successor continuation with it (via
/// [`decide_tail`]), so the re-classified step re-deposits the checker rather than dropping it —
/// this slot holds no contract of its own, so the ambient obligation is the whole winner.
pub(in crate::machine::execute) fn become_dispatch<'step>(
    view: &SchedulerView<'step, '_>,
    inner: KExpression<'step>,
) -> Outcome<'step> {
    Outcome::Continue {
        work: decide_tail(inner, view.current_obligation_duplicate()),
        frame: FramePlacement::Inherit,
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
    }
}

/// Walk raw parts emitting an `Identifier("")` placeholder at every
/// eager slot and a parallel staged-subs Vec; non-eager parts pass
/// through unchanged.
///
/// `wrap_indices` names bare-name value slots (the `wrap_indices` set from
/// [`KFunction::classify_for_pick`](crate::machine::core::KFunction::classify_for_pick))
/// to resolve before bind. The keyword path resolves these via `bare_outcomes`
/// because it must know their carried type *during* overload selection; the
/// post-pick named-argument / function-value tail has already committed to one
/// callable, so it resolves them by sub-Dispatch through the same eager-subs
/// parking/resume path as `Expression` parts. Callers with no committed pick
/// (the keyworded `Deferred` arm, which re-resolves on finish) pass `&[]`.
pub(super) fn stage_all_eager_parts<'step>(
    parts: Vec<Spanned<ExpressionPart<'step>>>,
    wrap_indices: &[usize],
) -> (
    Vec<Spanned<ExpressionPart<'step>>>,
    Vec<(usize, DepRequest<'step>)>,
) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'step>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, DepRequest<'step>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if wrap_indices.contains(&i) {
            // Bare-name value slot: resolve the name through a single-part
            // sub-Dispatch (the `BareIdentifier` / `BareTypeLeaf` fast lane), so
            // the resolved `Spliced` carrier reaches `accepts_part` at bind. Not
            // one of the six eager shapes (it wraps bare Identifier/Type parts),
            // so this stays a pre-check before the stager.
            let wrapped = KExpression::new(vec![Spanned {
                value: part.value,
                span,
            }]);
            staged.push((
                i,
                DepRequest::Dispatch {
                    expr: wrapped,
                    placement: DepPlacement::OwnScope,
                },
            ));
            new_parts.push(staged_slot_placeholder());
            continue;
        }
        match stage_eager_part(part.value) {
            Ok(dep) => {
                staged.push((i, dep));
                new_parts.push(staged_slot_placeholder());
            }
            Err(value) => new_parts.push(Spanned { value, span }),
        }
    }
    (new_parts, staged)
}

// ---------- Resume closure ----------

/// A dispatch slot's decide — the `SchedulerView -> Outcome` closure a dispatch [`NodeWork`](super::nodes::NodeWork) runs.
/// A birth decide classifies the carried `expr` (+ `pre_subs`) and routes; a park's resume re-runs
/// the decide its park captured (a bare leaf, an evolving `working_expr`). Boxing keeps the router
/// blind to which family it is — every `Wait` wakes through `run_step` uniformly.
pub(in crate::machine::execute) type ResumeFn<'step> =
    Box<dyn for<'view> FnOnce(&SchedulerView<'step, 'view>, usize) -> Outcome<'step> + 'step>;

// ---------- Cross-shape driver ----------

/// Build a birth dispatch [`NodeWork`](super::nodes::NodeWork) for `expr` with empty `pre_subs`, wrapping the
/// birth-dispatch continuation with the tail chain's declared-return `obligation` when one is
/// present (via [`with_obligation`], so the replacement step re-deposits the checker into the
/// ambient slot-step state before classifying — the keep-first capture that carries the first
/// caller's declared return down the chain). Pass `None` for a plain birth dispatch that carries no
/// inherited obligation.
pub(in crate::machine::execute) fn decide_tail<'step>(
    expr: KExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<KoanWorkload> {
    decide_with_presubs(expr, Vec::new(), obligation)
}

/// Birth dispatch [`NodeWork`](super::nodes::NodeWork) carrying the dispatch layer's pre-submitted nested sub-Dispatches
/// (computed by [`submit_expression`]). `obligation` wraps the birth-dispatch continuation before it
/// is boxed (the live wrap that must precede the [`NodeWork::new`] erase) so a tail replacement
/// carries its declared-return checker.
pub(in crate::machine::execute) fn decide_with_presubs<'step>(
    expr: KExpression<'step>,
    pre_subs: Vec<(usize, NodeId)>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<KoanWorkload> {
    let carrier = expr.summarize();
    // A birth decide waits on no deps: it runs on first poll, classifies, and routes.
    let continuation = ignore_results(Box::new(move |view, idx| {
        classify_dispatch(view, expr, pre_subs, idx)
    }));
    let continuation = match obligation {
        Some(obligation) => with_obligation(obligation, continuation),
        None => continuation,
    };
    NodeWork::new(ResolvedDeps::new(), continuation, Some(carrier))
}

/// Classify a freshly-born dispatch expression's shape and route to the matching per-shape decide,
/// returning the [`Outcome`] for the harness to apply. Fast-lane shapes terminalize or
/// single-producer-park in one poll; a shape that parks returns a `ParkThenContinue` whose resume
/// closure re-enters [`run_step`], never back through here.
fn classify_dispatch<'step>(
    view: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    pre_subs: Vec<(usize, NodeId)>,
    idx: usize,
) -> Outcome<'step> {
    match expr.shape() {
        DispatchShape::BareTypeLeaf => {
            debug_assert!(pre_subs.is_empty());
            let t = match &expr.parts[0].value {
                ExpressionPart::Type(t) => t.clone(),
                _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
            };
            view.with_current_scope(|s| single_poll::bare_type_leaf(view, s, &t))
        }
        DispatchShape::BareIdentifier => {
            debug_assert!(pre_subs.is_empty());
            let name = match &expr.parts[0].value {
                ExpressionPart::Identifier(n) => n.clone(),
                _ => unreachable!("BareIdentifier shape implies single Identifier part"),
            };
            view.with_current_scope(|s| single_poll::bare_identifier(view, s, name))
        }
        DispatchShape::FunctionValueCall => {
            debug_assert!(pre_subs.is_empty());
            fn_value::initial(view, expr)
        }
        DispatchShape::TypeCall => {
            debug_assert!(pre_subs.is_empty());
            single_poll::type_call(view, expr)
        }
        DispatchShape::HeadDeferred => {
            debug_assert!(pre_subs.is_empty());
            head_deferred::initial_expr(expr)
        }
        DispatchShape::TypeHeadDeferred => {
            debug_assert!(pre_subs.is_empty());
            head_deferred::initial_type(expr)
        }
        // Slot-terminal (TRY-catchable), uniform with every other dispatch failure —
        // a non-callable head is a runtime error, not a fatal `execute()` abort.
        DispatchShape::NonCallableHead => {
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
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
            debug_assert!(pre_subs.is_empty());
            view.with_current_scope(|s| operator_chain::run(view, s, &expr, idx))
        }
        DispatchShape::Keyworded => keyworded::initial(view, expr, pre_subs, idx),
        DispatchShape::SigiledTypeExpr => {
            debug_assert!(pre_subs.is_empty());
            single_poll::sigiled_type_expr(view, expr)
        }
        DispatchShape::RecordType => {
            debug_assert!(pre_subs.is_empty());
            single_poll::record_type(view, expr)
        }
        DispatchShape::LiteralPassThrough => {
            debug_assert!(pre_subs.is_empty());
            single_poll::literal_pass_through(view, expr)
        }
    }
}

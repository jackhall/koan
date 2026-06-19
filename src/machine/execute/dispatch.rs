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

use std::rc::Rc;

use crate::machine::core::CallFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::{Carried, Parseable};
use crate::machine::{KError, KErrorKind, NodeId, Resolution, Scope, TraceFrame};
use crate::source::Spanned;

use super::nodes::NodeWork;
use super::runtime::KoanWorkload;
use super::{ignore_results, DepFinish};
use crate::machine::core::kfunction::action::{DepPlacement, FramePlacement};
use crate::scheduler::Scheduler;

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

pub(in crate::machine::execute) use super::outcome::{Continuation, Outcome};
pub(in crate::machine::execute) use ctx::{reattach_node_scope, SchedulerView};
pub(crate) use field_list::defer_field_list_action;
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_dispatch::{NameOutcome, ResolveOutcome, Resolved};
pub use resolve_type_identifier::TypeIdentifierResolution;
pub(crate) use resolve_type_identifier::{resolve_type_leaf_carrier, TypeLeafCarrier};

/// The shape classification and classifier live in
/// [`crate::machine::model::ast`] (pure-structural, cached on the node at parse
/// time); re-exported here so dispatch-internal call sites and tests keep the
/// `dispatch::{DispatchShape, classify_dispatch_shape}` path.
#[allow(unused_imports)]
pub use crate::machine::model::ast::{classify_dispatch_shape, DispatchShape};

/// Resolve a bare-name `ExpressionPart` (Identifier or leaf Type)
/// against `scope`. `consumer = Some(idx)` enables the cycle check;
/// `consumer = None` skips it.
pub(super) fn resolve_name_part<'step>(
    scope: &Scope<'step>,
    part: &ExpressionPart<'step>,
    scheduler: &Scheduler<KoanWorkload>,
    active_chain: Option<&std::rc::Rc<crate::machine::LexicalFrame>>,
    consumer: Option<NodeId>,
) -> NameOutcome<'step> {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (n.as_str(), None),
        ExpressionPart::Type(t) => (t.as_str(), Some(t)),
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = active_chain.map(|c| &**c);
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
        Some(t) => match resolve_type_leaf_carrier(scope, t, active_chain.cloned()) {
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
fn disposition_for_producer<'step>(
    scheduler: &Scheduler<KoanWorkload>,
    name: &str,
    producer: NodeId,
    consumer: Option<NodeId>,
) -> NameOutcome<'step> {
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
pub(super) fn bare_name_of<'step>(part: &ExpressionPart<'step>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}

/// One staged submission queued by the keyworded part walk.
pub(in crate::machine::execute) enum PendingSub<'step> {
    Reuse(NodeId),
    Dispatch(KExpression<'step>),
    ListLit(Vec<ExpressionPart<'step>>),
    DictLit(Vec<(ExpressionPart<'step>, ExpressionPart<'step>)>),
    RecordLit(Vec<(String, ExpressionPart<'step>)>),
}

/// A dependency a [`Outcome::ParkThenContinue`] declares — the data the read-only decide phase hands
/// the harness (`KoanRuntime::apply_outcome`; the harness is the sole `&mut Scheduler` holder),
/// which runs the matching write. The
/// decide phase issues no graph write itself. `Dispatch` / `*Lit` / `BodyBlock` are fresh producers the harness submits
/// (and owns); `Existing` is a pre-existing producer the decide phase found that the slot merely
/// parks on. Deps resolve in declaration order, so a finish reads `results[k]` for the k-th dep —
/// except an `InScope`-placed `Dispatch` and a `BodyBlock`, whose multi-statement body each fan out
/// to one resolved producer per statement (the harness `extend`s them in order).
///
/// This enum names AST (`KExpression` / `ExpressionPart`) and so lives on the dispatch side, beside
/// [`PendingSub`]; [`Outcome`] carries it as an opaque type, keeping `outcome.rs` AST-free.
pub(in crate::machine::execute) enum DepRequest<'step> {
    Dispatch {
        expr: KExpression<'step>,
        placement: DepPlacement<'step>,
    },
    ListLit(Vec<ExpressionPart<'step>>),
    DictLit(Vec<(ExpressionPart<'step>, ExpressionPart<'step>)>),
    RecordLit(Vec<(String, ExpressionPart<'step>)>),
    /// A deferred-return FN's first-call body: dispatch `statements` (its non-tail body + the
    /// return-type expression, in that order) as body-chain siblings in the freshly acquired
    /// per-call `frame`, fanning out to one owned producer per statement. The combine reads the
    /// last (the resolved return type) to build the `PerCall` contract; the earlier statements'
    /// scope binds feed the tail body. The only dep that carries its own frame.
    BodyBlock {
        frame: Rc<CallFrame>,
        statements: Vec<KExpression<'step>>,
    },
    Existing(NodeId),
}

/// Result of a successful keyworded part walk.
pub(in crate::machine::execute) struct PartWalkResult<'step> {
    pub new_parts: Vec<Spanned<ExpressionPart<'step>>>,
    pub producers_to_wait: Vec<NodeId>,
    pub staged_subs: Vec<(usize, PendingSub<'step>)>,
}

/// The argument body of a `head (...)` / `head {...}` call, classified by surface shape.
///
/// - `Named` — a `{x = 1}` record literal: the sole named-argument surface (function and
///   functor calls, struct construction).
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

/// Park the slot on `deps` as a [`NodeWork`](super::nodes::NodeWork) whose
/// `finish` runs over their resolved values (the dispatch combine — short-circuits on dep error).
/// Every dep is owned (`park_count: 0`).
pub(in crate::machine::execute) fn park_on_deps<'step>(
    deps: Vec<DepRequest<'step>>,
    dep_error_frame: Option<TraceFrame>,
    finish: DepFinish<'step>,
) -> Outcome<'step> {
    Outcome::ParkThenContinue {
        deps,
        park_count: 0,
        continuation: Continuation::Finish(finish),
        dep_error_frame,
    }
}

/// Park the slot on `producers` (notify edges) and re-run its `resume` decide on wake — the
/// closure-carrying `ParkSelf` shape every park-and-replay family uses. `carrier` is the parked
/// expression's pre-rendered summary the deadlock report surfaces (`None` when the park carries no
/// renderable form); rendering it here keeps the AST out of the scheduler. The producers are the
/// to-wait set the decide already filtered.
pub(in crate::machine::execute) fn park_resume<'step>(
    producers: Vec<NodeId>,
    carrier: Option<String>,
    resume: ResumeFn<'step>,
) -> Outcome<'step> {
    Outcome::ParkThenContinue {
        park_count: producers.len(),
        deps: producers.into_iter().map(DepRequest::Existing).collect(),
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
/// expression to a nested one to re-classify (`(inner)`, `:(...)` unwrap).
pub(in crate::machine::execute) fn become_dispatch<'step>(
    inner: KExpression<'step>,
) -> Outcome<'step> {
    Outcome::Continue {
        work: decide(inner),
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
pub(super) fn stage_all_eager_parts<'step>(
    parts: Vec<Spanned<ExpressionPart<'step>>>,
    wrap_indices: &[usize],
) -> (
    Vec<Spanned<ExpressionPart<'step>>>,
    Vec<(usize, PendingSub<'step>)>,
) {
    let mut new_parts: Vec<Spanned<ExpressionPart<'step>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, PendingSub<'step>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if wrap_indices.contains(&i) {
            // Bare-name value slot: resolve the name through a single-part
            // sub-Dispatch (the `BareIdentifier` / `BareTypeLeaf` fast lane), so
            // the resolved `Spliced` carrier reaches `accepts_part` at bind.
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

// ---------- Resume closure ----------

/// A dispatch slot's decide — the `SchedulerView -> Outcome` closure a dispatch [`NodeWork`](super::nodes::NodeWork) runs.
/// A birth decide classifies the carried `expr` (+ `pre_subs`) and routes; a park's resume re-runs
/// the decide its park captured (a bare leaf, an evolving `working_expr`). Boxing keeps the router
/// blind to which family it is — every `Wait` wakes through `run_step` uniformly.
pub(in crate::machine::execute) type ResumeFn<'step> =
    Box<dyn for<'view> FnOnce(&SchedulerView<'step, 'view>, usize) -> Outcome<'step> + 'step>;

// ---------- Cross-shape driver ----------

/// Build a birth dispatch [`NodeWork`](super::nodes::NodeWork) for `expr` with empty `pre_subs` — the dispatch-layer
/// constructor every tail-replace / re-dispatch site uses instead of a raw work literal. The
/// captured closure classifies `expr` on first poll; `carrier` is its deadlock-summary.
pub(in crate::machine::execute) fn decide<'step>(
    expr: KExpression<'step>,
) -> NodeWork<KoanWorkload> {
    decide_with_presubs(expr, Vec::new())
}

/// Birth dispatch [`NodeWork`](super::nodes::NodeWork) carrying the dispatch layer's pre-submitted nested sub-Dispatches
/// (computed by [`submit_expression`]).
pub(in crate::machine::execute) fn decide_with_presubs<'step>(
    expr: KExpression<'step>,
    pre_subs: Vec<(usize, NodeId)>,
) -> NodeWork<KoanWorkload> {
    let carrier = expr.summarize();
    // A birth decide waits on no deps and ignores the (empty) results slice; it runs on first poll,
    // classifies, and routes. `ignore_results` adapts the decide closure to the unified `NodeContinuation`.
    NodeWork::new(
        Vec::new(),
        0,
        ignore_results(Box::new(move |view, idx| {
            classify_dispatch(view, expr, pre_subs, idx)
        })),
        Some(carrier),
    )
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
            single_poll::bare_type_leaf(view, &t)
        }
        DispatchShape::BareIdentifier => {
            debug_assert!(pre_subs.is_empty());
            let name = match &expr.parts[0].value {
                ExpressionPart::Identifier(n) => n.clone(),
                _ => unreachable!("BareIdentifier shape implies single Identifier part"),
            };
            single_poll::bare_identifier(view, name)
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
            operator_chain::run(view, &expr)
        }
        DispatchShape::Keyworded => keyworded::initial(view, expr, pre_subs, idx),
        DispatchShape::SigiledTypeExpr => {
            debug_assert!(pre_subs.is_empty());
            single_poll::sigiled_type_expr(expr)
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

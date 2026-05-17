//! FN return‑type pipeline: extraction → classification → carriage across the
//! Combine boundary → resolution at finish time.
//!
//! Three enums and four functions covering one concern: the return‑type slot's
//! lifecycle from the raw bundle entry to a final [`ReturnType`] on the
//! registered `KFunction`'s signature.
//!
//! Stage A (FN‑def time):
//! - [`ReturnTypeRaw`] — what the bundle gave us, before any classification.
//!   [`extract_return_type_raw`] reads the slot and classifies its carrier shape.
//! - [`ReturnTypeState`] — the post‑classification outcome.
//!   [`classify_return_type`] applies the Stage B parameter‑name scan and either
//!   resolves (synchronous), parks (forward‑ref), defers (parameter reference),
//!   or sub‑dispatches (no‑param parens‑form expression).
//!
//! Stage B (Combine boundary, only on Pending/SubDispatched paths):
//! - [`ReturnTypeCapture`] — the carrier that survives the Combine wait.
//!   [`resolve_capture_at_finish`] re‑elaborates against the now‑final scope.

use crate::machine::core::ResolveTypeExprOutcome;
use crate::machine::core::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::machine::model::ast::{KExpression, TypeExpr, TypeParams};
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, KError, KErrorKind, NodeId, Scope, SchedulerHandle};

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// The return‑type slot accepts three carrier shapes (matching the two FN overloads):
///
///  * `Resolved(kt)` — `KObject::KTypeValue(kt)`, overload 1's eager‑resolved leaf
///    or structural type (`Number`, `List<Str>`, `(SIG_WITH Set ((Elt: Number)))`
///    after construction‑time sub‑Dispatch). Lifts directly into `Resolved(kt)`.
///  * `TypeExprCarrier(te)` — `KObject::TypeNameRef(t, _)`, overload 1's bare‑leaf
///    carrier the parser created because the name isn't in `KType::from_name`'s
///    table (`Point`, `IntOrd`, `MyList`). Walked by the elaborator downstream.
///  * `ExprCarrier(e)` — `KObject::KExpression(e)`, overload 2 (Stage B)
///    parens‑form return type (`(MODULE_TYPE_OF Er Type)`,
///    `(SIG_WITH Set ((Elt: Er)))`) captured raw so the expression survives FN‑def
///    without sub‑dispatching against the outer scope where the parameter is
///    unbound by construction.
pub(super) enum ReturnTypeRaw<'a> {
    Resolved(KType),
    TypeExprCarrier(TypeExpr),
    ExprCarrier(KExpression<'a>),
}

/// Post‑classification outcome of the return‑type slot. Routed in
/// [`super::body`]'s final match against the parameter‑list outcome.
///
///  * `Done(kt)` — fully resolved at FN‑def time (most common case).
///  * `Pending { te, producers }` — bare‑leaf elaboration parked on a placeholder
///    (forward‑LET case); resumed via Combine wake against the now‑final scope.
///  * `Deferred(_)` — parameter‑name leaf detected in the carrier; the per‑call
///    elaboration runs at the dispatch boundary (see `KFunction::invoke`).
///    Skips the outer‑scope elaborator entirely — running it would surface an
///    `Unbound` because the parameter is by construction not in the FN's lexical
///    scope.
///  * `ExprSubDispatched(id)` — overload 2's no‑parameter‑reference path: the
///    return‑type expression sub‑dispatched at FN‑def time. The Combine finish
///    reads the result from `results[<id‑position>]` and lifts into `Resolved`.
pub(super) enum ReturnTypeState<'a> {
    Done(KType),
    Pending { te: TypeExpr, producers: Vec<NodeId> },
    Deferred(DeferredReturn<'a>),
    ExprSubDispatched(NodeId),
}

/// Carrier for the return type across the Combine boundary. `Resolved` means we
/// already have a concrete `KType` and the Combine finish skips re‑elaboration;
/// `Unresolved` means we parked on a bare leaf name and the finish runs
/// `elaborate_type_expr` against the now‑final scope. `TypeExpr` is the
/// structured variant — used when the parser‑preserved `TypeExpr` carries
/// non‑trivial parameter structure (`List<MyT>`, `Function<(A) -> B>`,
/// `Foo<Bar, Baz>`) whose `TypeParams::List` / `TypeParams::Function` slots need
/// to be preserved verbatim for re‑elaboration against the now‑final scope.
/// Plumbing the full `TypeExpr` rather than just the leaf name keeps the `params`
/// intact; rendering and re‑parsing would round‑trip through a string and strip
/// the structure.
///
/// Parens‑wrapped return‑type expressions like `(SIG_WITH SetSig ((Elt: Number)))`
/// do NOT route through this carrier in the today's wrap‑path. The dispatcher's
/// eager‑sub‑dispatch path resolves them at FN‑construction time and splices the
/// resulting `KTypeValue` into the FN bundle as a `Future(_)`; the FN body then
/// extracts a concrete `KType` via the `Resolved` arm. The structured‑`TypeExpr`
/// carrier exists for parked‑during‑construction leaf‑with‑parameters shapes
/// where the parser already produced a `TypeExpr` with non‑`None` params and we
/// need to wait on a type‑binding placeholder before final elaboration.
pub(super) enum ReturnTypeCapture<'a> {
    Resolved(KType),
    Unresolved(String),
    TypeExpr(TypeExpr),
    /// Module‑system functor‑params Stage B: parameter‑name reference detected in
    /// the return‑type carrier at FN‑def time. The carrier is held verbatim and
    /// propagated through to the final `ReturnType::Deferred(_)` on the
    /// registered `KFunction`'s signature without elaboration at the Combine
    /// wake — per‑call elaboration runs at the dispatch boundary instead.
    Deferred(DeferredReturn<'a>),
    /// Module‑system functor‑params Stage B: overload‑2 return‑type carrier whose
    /// parens‑form expression doesn't reference any parameter — sub‑dispatch the
    /// expression at FN‑def and lift the resulting `KTypeValue` into `Resolved`
    /// at Combine finish. The `results_pos` index says where the result lands in
    /// the closure's `&[&'a KObject<'a>]` slice; the FN‑def body computes this
    /// when it merges the return‑type sub‑dispatch into the Combine's overall
    /// `deps` order.
    ReturnTypeExpr { results_pos: usize },
}

/// Read the `return_type` slot from `bundle` and classify its carrier shape.
/// Returns `Err` only on the structural rejection path (no recognized carrier).
/// `unreachable!` arms guard the `get(kind) → extract_kind` pairing — those are
/// internal invariants of [`ArgumentBundle`], not user‑surface errors.
pub(super) fn extract_return_type_raw<'a>(
    bundle: &mut ArgumentBundle<'a>,
) -> Result<ReturnTypeRaw<'a>, KError> {
    match bundle.get("return_type") {
        Some(KObject::KTypeValue(_)) => match extract_ktype(bundle, "return_type") {
            Some(t) => Ok(ReturnTypeRaw::Resolved(t)),
            None => unreachable!("get(KTypeValue) then extract_ktype must succeed"),
        },
        Some(KObject::TypeNameRef(_)) => match extract_type_name_ref(bundle, "return_type") {
            Some(te) => Ok(ReturnTypeRaw::TypeExprCarrier(te)),
            None => unreachable!("get(TypeNameRef) then extract_type_name_ref must succeed"),
        },
        Some(KObject::KExpression(_)) => match extract_kexpression(bundle, "return_type") {
            Some(e) => Ok(ReturnTypeRaw::ExprCarrier(e)),
            None => unreachable!("get(KExpression) then extract_kexpression must succeed"),
        },
        _ => Err(KError::new(KErrorKind::ShapeError(
            "FN return-type slot must be a type expression (e.g. `Number`, `:(List Str)`)"
                .to_string(),
        ))),
    }
}

/// Route `raw` to one of four [`ReturnTypeState`] outcomes. Applies the Stage B
/// parameter‑name scan first (a match short‑circuits eager elaboration), then
/// resolves the carrier against `scope` or — for the no‑param expression
/// carrier — schedules a sub‑Dispatch on `sched`.
pub(super) fn classify_return_type<'a>(
    raw: ReturnTypeRaw<'a>,
    param_names: &[String],
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
) -> Result<ReturnTypeState<'a>, KError> {
    match raw {
        ReturnTypeRaw::Resolved(kt) => Ok(ReturnTypeState::Done(kt)),
        ReturnTypeRaw::TypeExprCarrier(te) => {
            if type_expr_references_any(&te, param_names) {
                return Ok(ReturnTypeState::Deferred(DeferredReturn::TypeExpr(te)));
            }
            let name = te.name.clone();
            Ok(match scope.resolve_type_expr(&te) {
                ResolveTypeExprOutcome::Done(kt) => ReturnTypeState::Done(kt.clone()),
                ResolveTypeExprOutcome::Park(producers) => {
                    ReturnTypeState::Pending { te, producers }
                }
                ResolveTypeExprOutcome::Unbound(_) => match KType::from_name(&name) {
                    Some(kt) => ReturnTypeState::Done(kt),
                    None => {
                        return Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot = unknown type name `{name}`"
                        ))));
                    }
                },
            })
        }
        ReturnTypeRaw::ExprCarrier(e) => {
            if kexpression_references_any(&e, param_names) {
                Ok(ReturnTypeState::Deferred(DeferredReturn::Expression(e)))
            } else {
                let id = sched.add_dispatch(e, scope);
                Ok(ReturnTypeState::ExprSubDispatched(id))
            }
        }
    }
}

/// Pick the right [`ReturnTypeCapture`] variant for a parked‑during‑construction
/// `TypeExpr`. Bare leaves (`Point`, `IntOrd`) route through `Unresolved(name)`
/// so the legacy `KType::from_name` fast path applies on the Combine wake.
/// Parameterized shapes (`List<MyT>`, `Foo<Bar, Baz>`) route through
/// `TypeExpr(te)` so the structured elaboration survives the boundary verbatim.
pub(super) fn make_capture<'a>(te: TypeExpr) -> ReturnTypeCapture<'a> {
    match te.params {
        TypeParams::None => ReturnTypeCapture::Unresolved(te.name),
        TypeParams::List(_) | TypeParams::Function { .. } => ReturnTypeCapture::TypeExpr(te),
    }
}

/// Resolve a [`ReturnTypeCapture`] to a final [`ReturnType`] at Combine‑finish
/// time. The closure body in `defer_via_combine` calls this once the parking
/// producers have settled and the spliced signature is ready.
///
/// Park‑arm outcomes from [`Scope::resolve_type_expr`] are *protocol errors* at
/// this point — every parked producer is terminal by the Combine‑finish
/// invariant; a second park would loop forever, so we surface it as a structured
/// error instead.
pub(super) fn resolve_capture_at_finish<'a>(
    capture: ReturnTypeCapture<'a>,
    scope: &'a Scope<'a>,
    results: &[&'a KObject<'a>],
) -> Result<ReturnType<'a>, KError> {
    match capture {
        ReturnTypeCapture::Resolved(kt) => Ok(ReturnType::Resolved(kt)),
        ReturnTypeCapture::Unresolved(name) => {
            let te = TypeExpr::leaf(name.clone());
            match scope.resolve_type_expr(&te) {
                ResolveTypeExprOutcome::Done(kt) => Ok(ReturnType::Resolved(kt.clone())),
                ResolveTypeExprOutcome::Park(_) => Err(KError::new(KErrorKind::ShapeError(
                    "FN return type parked after Combine wake".to_string(),
                ))),
                ResolveTypeExprOutcome::Unbound(_) => match KType::from_name(&name) {
                    Some(kt) => Ok(ReturnType::Resolved(kt)),
                    None => Err(KError::new(KErrorKind::ShapeError(format!(
                        "FN return-type slot = unknown type name `{name}`"
                    )))),
                },
            }
        }
        ReturnTypeCapture::TypeExpr(t) => match scope.resolve_type_expr(&t) {
            ResolveTypeExprOutcome::Done(kt) => Ok(ReturnType::Resolved(kt.clone())),
            ResolveTypeExprOutcome::Park(_) => Err(KError::new(KErrorKind::ShapeError(
                "FN return type parked after Combine wake".to_string(),
            ))),
            ResolveTypeExprOutcome::Unbound(msg) => Err(KError::new(KErrorKind::ShapeError(
                format!("FN return-type slot: {msg}"),
            ))),
        },
        ReturnTypeCapture::Deferred(d) => Ok(ReturnType::Deferred(d)),
        ReturnTypeCapture::ReturnTypeExpr { results_pos } => {
            let obj = results[results_pos];
            match obj {
                KObject::KTypeValue(kt) => Ok(ReturnType::Resolved(kt.clone())),
                other => Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN return-type slot sub-Dispatch expected a type expression, \
                     got a {} value",
                    other.ktype().name(),
                )))),
            }
        }
    }
}

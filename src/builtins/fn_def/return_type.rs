//! FN return‑type pipeline: extraction → classification → carriage across the
//! Combine boundary → resolution at finish time.
//!
//! Stage A (FN‑def time): [`ReturnTypeRaw`] → [`ReturnTypeState`].
//! Stage B (Combine boundary, only on Pending / SubDispatched paths):
//! [`ReturnTypeCapture`] → [`ReturnType`].

use crate::machine::core::ResolveTypeExprOutcome;
use crate::machine::core::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::machine::model::ast::{KExpression, TypeExpr, TypeParams};
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, KError, KErrorKind, NodeId, Scope, SchedulerHandle};

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// Carrier shape of the return‑type slot, before any classification.
///
/// `ExprCarrier` is captured raw rather than sub‑dispatched in the outer scope
/// because overload 2's expression may reference a parameter that is by
/// construction unbound there.
pub(super) enum ReturnTypeRaw<'a> {
    Resolved(KType),
    TypeExprCarrier(TypeExpr),
    ExprCarrier(KExpression<'a>),
}

/// Post‑classification outcome of the return‑type slot.
///
/// `Deferred` skips the outer‑scope elaborator entirely: running it would
/// surface an `Unbound` because the referenced parameter is by construction
/// not in the FN's lexical scope. Per‑call elaboration runs at the dispatch
/// boundary instead.
pub(super) enum ReturnTypeState<'a> {
    Done(KType),
    Pending { te: TypeExpr, producers: Vec<NodeId> },
    Deferred(DeferredReturn<'a>),
    ExprSubDispatched(NodeId),
}

/// Carrier for the return type across the Combine boundary.
///
/// `TypeExpr` plumbs the structured form (rather than just the leaf name) so
/// that `TypeParams::List` / `TypeParams::Function` survive verbatim — rendering
/// and re‑parsing would round‑trip through a string and strip the structure.
pub(super) enum ReturnTypeCapture<'a> {
    Resolved(KType),
    Unresolved(String),
    TypeExpr(TypeExpr),
    Deferred(DeferredReturn<'a>),
    /// `results_pos` indexes the Combine closure's `&[&'a KObject<'a>]` slice.
    ReturnTypeExpr { results_pos: usize },
}

/// `unreachable!` arms guard the `get(kind) → extract_kind` pairing — an
/// internal invariant of [`ArgumentBundle`], not a user‑surface error.
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

/// The parameter‑name scan runs first: a match short‑circuits eager
/// elaboration so the carrier survives verbatim to the dispatch boundary.
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
                ResolveTypeExprOutcome::Unbound(msg) => match KType::from_name(&name) {
                    Some(kt) => ReturnTypeState::Done(kt),
                    None => {
                        return Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot: {msg}"
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

pub(super) fn make_capture<'a>(te: TypeExpr) -> ReturnTypeCapture<'a> {
    match te.params {
        TypeParams::None => ReturnTypeCapture::Unresolved(te.name),
        TypeParams::List(_) | TypeParams::Function { .. } => ReturnTypeCapture::TypeExpr(te),
    }
}

/// Park‑arm outcomes from [`Scope::resolve_type_expr`] are *protocol errors*
/// here — every parked producer is terminal by the Combine‑finish invariant;
/// a second park would loop forever, so it is surfaced as a structured error.
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
                ResolveTypeExprOutcome::Unbound(msg) => match KType::from_name(&name) {
                    Some(kt) => Ok(ReturnType::Resolved(kt)),
                    None => Err(KError::new(KErrorKind::ShapeError(format!(
                        "FN return-type slot: {msg}"
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

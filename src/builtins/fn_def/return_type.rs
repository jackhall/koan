//! FN return‑type pipeline: extraction → classification → carriage across the
//! Combine boundary → resolution at finish time.
//!
//! Stage A (FN‑def time): [`ReturnTypeRaw`] → [`ReturnTypeState`].
//! Stage B (Combine boundary, only on Pending / SubDispatched paths):
//! [`ReturnTypeCapture`] → [`ReturnType`].

use std::collections::HashMap;

use crate::machine::core::ResolveTypeExprOutcome;
use crate::machine::core::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, KError, KErrorKind, NodeId, Scope};

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// Carrier shape of the return‑type slot, before any classification.
///
/// `ExprCarrier` is captured raw rather than sub‑dispatched in the outer scope
/// because overload 2's expression may reference a parameter that is by
/// construction unbound there.
pub(crate) enum ReturnTypeRaw<'a> {
    Resolved(KType<'a>),
    TypeExprCarrier(TypeExpr),
    ExprCarrier(KExpression<'a>),
}

/// Post‑classification outcome of the return‑type slot.
///
/// `Deferred` skips the outer‑scope elaborator entirely: running it would
/// surface an `Unbound` because the referenced parameter is by construction
/// not in the FN's lexical scope. Per‑call elaboration runs at the dispatch
/// boundary instead.
pub(crate) enum ReturnTypeState<'a> {
    Done(KType<'a>),
    Pending { te: TypeExpr, producers: Vec<NodeId> },
    Deferred(DeferredReturn<'a>),
    /// The return-type slot is an `Expression(_)` carrier (e.g. `-> (Mo.Ty)`)
    /// that doesn't reference any FN parameter, so it's safe to resolve once at
    /// FN-def time. The actual `add_dispatch` is deferred to
    /// [`super::finalize::defer_via_combine`] so all owned-sub scheduling
    /// happens at one site; param-type sub-dispatches in
    /// [`super::signature::ParamListOutcome::Pending::sub_dispatches`] follow
    /// the same defer-and-schedule pattern.
    ExprToSubDispatch(KExpression<'a>),
}

/// Carrier for the return type across the Combine boundary.
///
/// `TypeExpr` plumbs the structured form (rather than just the leaf name) so
/// that `TypeParams::List` / `TypeParams::Function` survive verbatim — rendering
/// and re‑parsing would round‑trip through a string and strip the structure.
pub(crate) enum ReturnTypeCapture<'a> {
    Resolved(KType<'a>),
    Unresolved(String),
    TypeExpr(TypeExpr),
    Deferred(DeferredReturn<'a>),
    /// `results_pos` indexes the Combine closure's `&[&'a KObject<'a>]` slice.
    ReturnTypeExpr { results_pos: usize },
}

/// `unreachable!` arms guard the `get(kind) → extract_kind` pairing — an
/// internal invariant of [`ArgumentBundle`], not a user‑surface error.
pub(crate) fn extract_return_type_raw<'a>(
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

/// FUNCTOR-return admissibility verdict, emitted alongside the
/// [`ReturnTypeState`] by [`classify_return_type`]. FN paths pass
/// `functor_param_types: None` and ignore the verdict; FUNCTOR paths pass
/// the param-name → declared-`KType` map so the deferred-arm verdict resolves
/// in the same walk.
pub(crate) enum AdmissibleVerdict {
    /// Carrier is admissible at classification time — either the resolved
    /// `KType` passes [`KType::is_admissible_functor_return`] or the deferred
    /// surface form passes the head inspector.
    Admissible,
    /// Final admissibility check rides Combine-finish. `Pending` and
    /// `ExprToSubDispatch` carriers can't be authoritatively classified until
    /// the resolved `KType` is in hand; the `is_functor: true` flag threaded
    /// through `defer_via_combine` re-runs the predicate then.
    DeferredToCombine,
    /// Carrier is definitively rejected at classification time. The error
    /// carries the diagnostic already formatted with the `FUNCTOR return-type
    /// slot` prefix.
    Rejected(KError),
}

/// Fused walk: classify the carrier (Resolved/Pending/Deferred/Expression)
/// *and* emit the FUNCTOR-return admissibility verdict in one pass.
///
/// The parameter‑name scan runs first: a match short‑circuits eager
/// elaboration so the carrier survives verbatim to the dispatch boundary.
/// The admissibility verdict is computed in the same arm — `Done` against
/// the elaborated `KType`, `Deferred` against the surface form, with
/// `Pending` / `ExprToSubDispatch` deferring to Combine-finish.
///
/// `functor_param_types`: `None` for FN (verdict computation is skipped and
/// `AdmissibleVerdict::Admissible` is returned as a no-op); `Some(&map)` for
/// FUNCTOR (param-name → declared-`KType` map drives the deferred-arm
/// bare-leaf type-denoting check).
pub(crate) fn classify_return_type<'a>(
    raw: ReturnTypeRaw<'a>,
    param_names: &[String],
    scope: &'a Scope<'a>,
    functor_param_types: Option<&HashMap<String, KType<'a>>>,
) -> Result<(ReturnTypeState<'a>, AdmissibleVerdict), KError> {
    match raw {
        ReturnTypeRaw::Resolved(kt) => {
            let verdict = verdict_for_resolved(&kt, functor_param_types.is_some());
            Ok((ReturnTypeState::Done(kt), verdict))
        }
        ReturnTypeRaw::TypeExprCarrier(te) => {
            if type_expr_references_any(&te, param_names) {
                let verdict = match functor_param_types {
                    Some(map) => verdict_for_deferred_type_expr(&te, map),
                    None => AdmissibleVerdict::Admissible,
                };
                return Ok((ReturnTypeState::Deferred(DeferredReturn::TypeExpr(te)), verdict));
            }
            let name = te.name.clone();
            let state = match scope.resolve_type_expr(&te) {
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
            };
            let verdict = match &state {
                ReturnTypeState::Done(kt) => {
                    verdict_for_resolved(kt, functor_param_types.is_some())
                }
                _ => AdmissibleVerdict::DeferredToCombine,
            };
            Ok((state, verdict))
        }
        ReturnTypeRaw::ExprCarrier(e) => {
            if kexpression_references_any(&e, param_names) {
                let verdict = match functor_param_types {
                    Some(_) => verdict_for_deferred_expression(&e),
                    None => AdmissibleVerdict::Admissible,
                };
                Ok((ReturnTypeState::Deferred(DeferredReturn::Expression(e)), verdict))
            } else {
                Ok((ReturnTypeState::ExprToSubDispatch(e), AdmissibleVerdict::DeferredToCombine))
            }
        }
    }
}

/// Resolved-arm verdict: runs `is_admissible_functor_return` against the
/// elaborated `KType`. FN callers pass `is_functor=false` and get
/// `Admissible` back unconditionally (the verdict is ignored on that path).
fn verdict_for_resolved<'a>(kt: &KType<'a>, is_functor: bool) -> AdmissibleVerdict {
    if !is_functor || kt.is_admissible_functor_return() {
        AdmissibleVerdict::Admissible
    } else {
        AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(format!(
            "FUNCTOR return-type slot must denote a module, signature, or functor; got `{}`",
            kt.name(),
        ))))
    }
}

/// Deferred-arm verdict for a `TypeExpr` carrier. A bare-leaf `Er` matching
/// a parameter name admits iff that parameter's declared `KType` is
/// type-denoting (e.g. `:OrderedSig`, `:Module`). A `Functor`-headed
/// parameterized form admits via the type-position sigil. Other shapes are
/// rejected here so the diagnostic surfaces at the FUNCTOR site.
fn verdict_for_deferred_type_expr<'a>(
    te: &TypeExpr,
    param_type_map: &HashMap<String, KType<'a>>,
) -> AdmissibleVerdict {
    match &te.params {
        TypeParams::None => {
            // Bare-leaf reference. If it matches a parameter name, admit iff
            // the parameter's declared type is type-denoting. Otherwise the
            // map was empty for this name (param-type slot didn't elaborate
            // eagerly); admit conservatively — downstream resolution
            // surfaces a structured error if the carrier is invalid.
            if let Some(param_kt) = param_type_map.get(&te.name) {
                if param_kt.is_type_denoting() {
                    AdmissibleVerdict::Admissible
                } else {
                    AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(format!(
                        "FUNCTOR return-type slot must denote a module, signature, or functor; \
                         parameter `{}` is declared as `{}`, which is not type-denoting",
                        te.name,
                        param_kt.name(),
                    ))))
                }
            } else {
                AdmissibleVerdict::Admissible
            }
        }
        TypeParams::Function { .. } if te.name == "Functor" => AdmissibleVerdict::Admissible,
        TypeParams::Function { .. } | TypeParams::List(_) => {
            AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(format!(
                "FUNCTOR return-type slot must denote a module, signature, or functor; got `{}`",
                te.render(),
            ))))
        }
    }
}

/// Deferred-arm verdict for a parens-form return-type carrier (`(SIG_WITH …)`,
/// `(MODULE_TYPE_OF …)`, etc). Head-keyword classification: `SIG_WITH` →
/// admissible (yields `SatisfiesSignature`); `MODULE_TYPE_OF` → rejected
/// (yields `AbstractType`). Other heads fall through to a generic rejection.
fn verdict_for_deferred_expression(e: &KExpression<'_>) -> AdmissibleVerdict {
    let head_keyword = e.parts.iter().find_map(|p| match &p.value {
        ExpressionPart::Keyword(s) => Some(s.as_str()),
        _ => None,
    });
    match head_keyword {
        Some("SIG_WITH") => AdmissibleVerdict::Admissible,
        Some("MODULE_TYPE_OF") => AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             `MODULE_TYPE_OF` produces an abstract type, not a module or signature"
                .to_string(),
        ))),
        Some(other) => AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(format!(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             head keyword `{other}` does not produce a module, signature, or functor",
        )))),
        None => AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             return-type expression has no recognizable head"
                .to_string(),
        ))),
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

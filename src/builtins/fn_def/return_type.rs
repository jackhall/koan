//! FN return-type pipeline: extraction → classification → carriage across the
//! Combine boundary → resolution at finish time.

use std::collections::HashMap;

use crate::machine::core::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeName};
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::{KObject, KType};
use crate::machine::ResolveTypeExprOutcome;
use crate::machine::{ArgumentBundle, KError, KErrorKind, NodeId, Scope};
use std::rc::Rc;

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// `ExprCarrier` is captured raw rather than sub-dispatched in the outer scope because
/// overload 2's expression may reference a parameter that is unbound there.
pub(crate) enum ReturnTypeRaw<'a> {
    Resolved(KType<'a>),
    TypeExprCarrier(TypeName),
    ExprCarrier(KExpression<'a>),
}

/// `Deferred` skips the outer-scope elaborator entirely: running it would surface
/// `Unbound` because the referenced parameter is not in the FN's lexical scope.
/// Per-call elaboration runs at the dispatch boundary instead.
pub(crate) enum ReturnTypeState<'a> {
    Done(KType<'a>),
    Pending {
        te: TypeName,
        producers: Vec<NodeId>,
    },
    Deferred(DeferredReturn<'a>),
    /// `Expression(_)` carrier (e.g. `-> (Mo.Ty)`) that doesn't reference any FN
    /// parameter; safe to resolve once at FN-def time. Scheduling happens via
    /// `super::finalize::defer_via_combine` so all owned-sub registration lives
    /// at one site.
    ExprToSubDispatch(KExpression<'a>),
}

/// `TypeExpr` plumbs the structured form verbatim so a re-elaboration sees the same
/// surface shape — rendering and re-parsing would strip it.
pub(crate) enum ReturnTypeCapture<'a> {
    Resolved(KType<'a>),
    Unresolved(String),
    TypeExpr(TypeName),
    Deferred(DeferredReturn<'a>),
    /// `results_pos` indexes the Combine closure's `&[&'a KObject<'a>]` slice.
    ReturnTypeExpr {
        results_pos: usize,
    },
}

/// `unreachable!` arms guard the `get(kind) → extract_kind` pairing — an internal
/// `ArgumentBundle` invariant, not a user-surface error.
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
            "FN return-type slot must be a type expression (e.g. `Number`, `:(LIST OF Str)`)"
                .to_string(),
        ))),
    }
}

/// FUNCTOR-return admissibility verdict. FN paths pass `functor_param_types: None` and
/// ignore the verdict; FUNCTOR paths pass the param-name → declared-`KType` map so the
/// deferred-arm verdict resolves in the same walk.
pub(crate) enum AdmissibleVerdict {
    Admissible,
    /// `Pending` and `ExprToSubDispatch` carriers can't be classified until the resolved
    /// `KType` is in hand; the `is_functor: true` flag threaded through
    /// `defer_via_combine` re-runs the predicate at Combine-finish.
    DeferredToCombine,
    /// Diagnostic is already formatted with the `FUNCTOR return-type slot` prefix.
    Rejected(KError),
}

/// Fused walk: classify the carrier and emit the FUNCTOR-return admissibility verdict
/// in one pass. The parameter-name scan runs first so a match short-circuits eager
/// elaboration and the carrier survives verbatim to the dispatch boundary.
///
/// `functor_param_types`: `None` for FN (verdict skipped, `Admissible` returned as a
/// no-op); `Some(&map)` for FUNCTOR (drives the deferred-arm bare-leaf type-denoting
/// check).
pub(crate) fn classify_return_type<'a>(
    raw: ReturnTypeRaw<'a>,
    param_names: &[String],
    scope: &'a Scope<'a>,
    chain: Option<Rc<LexicalFrame>>,
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
                return Ok((
                    ReturnTypeState::Deferred(DeferredReturn::TypeExpr(te)),
                    verdict,
                ));
            }
            let name = te.render();
            // Gated to the FN's lexical position — a return type naming a later type is a
            // position error, like any other forward reference.
            let state = match scope.resolve_type_expr(&te, chain) {
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
                Ok((
                    ReturnTypeState::Deferred(DeferredReturn::Expression(e)),
                    verdict,
                ))
            } else {
                Ok((
                    ReturnTypeState::ExprToSubDispatch(e),
                    AdmissibleVerdict::DeferredToCombine,
                ))
            }
        }
    }
}

/// FN callers pass `is_functor=false` and get `Admissible` back unconditionally.
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

/// Bare-leaf `Er` matching a parameter name admits iff that parameter's declared
/// `KType` is type-denoting (e.g. `:OrderedSig`, `:Module`). A `Functor`-headed
/// parameterized form admits via the type-position sigil; other shapes are rejected
/// so the diagnostic surfaces at the FUNCTOR site.
fn verdict_for_deferred_type_expr<'a>(
    te: &TypeName,
    param_type_map: &HashMap<String, KType<'a>>,
) -> AdmissibleVerdict {
    // Map miss means the param-type slot didn't elaborate eagerly; admit
    // conservatively and let downstream resolution surface any structured error.
    if let Some(param_kt) = param_type_map.get(te.as_str()) {
        if param_kt.is_type_denoting() {
            AdmissibleVerdict::Admissible
        } else {
            AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(format!(
                "FUNCTOR return-type slot must denote a module, signature, or functor; \
                 parameter `{}` is declared as `{}`, which is not type-denoting",
                te.as_str(),
                param_kt.name(),
            ))))
        }
    } else {
        AdmissibleVerdict::Admissible
    }
}

/// Head-keyword classification for deferred return-type carriers: `WITH` (a `sig WITH
/// {…}` specialization) admits (yields `Signature { .. }`); a dotted `ATTR` head
/// (`Er.Type`, a module type-member access) rejects (yields `AbstractType`); other heads
/// fall through to a generic rejection.
fn verdict_for_deferred_expression(e: &KExpression<'_>) -> AdmissibleVerdict {
    let head_keyword = e.parts.iter().find_map(|p| match &p.value {
        ExpressionPart::Keyword(s) => Some(s.as_str()),
        _ => None,
    });
    match head_keyword {
        Some("WITH") => AdmissibleVerdict::Admissible,
        Some("ATTR") => AdmissibleVerdict::Rejected(KError::new(KErrorKind::ShapeError(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             a module type-member access (`Er.Type`) produces an abstract type, \
             not a module or signature"
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

pub(super) fn make_capture<'a>(te: TypeName) -> ReturnTypeCapture<'a> {
    ReturnTypeCapture::Unresolved(te.render())
}

/// Park-arm outcomes from `Scope::resolve_type_expr` are protocol errors here: every
/// parked producer is terminal by the Combine-finish invariant, so a second park would
/// loop forever and is surfaced as a structured error.
pub(super) fn resolve_capture_at_finish<'a>(
    capture: ReturnTypeCapture<'a>,
    scope: &'a Scope<'a>,
    results: &[&'a KObject<'a>],
) -> Result<ReturnType<'a>, KError> {
    match capture {
        ReturnTypeCapture::Resolved(kt) => Ok(ReturnType::Resolved(kt)),
        ReturnTypeCapture::Unresolved(name) => {
            let te = TypeName::leaf(name.clone());
            match scope.resolve_type_expr(&te, None) {
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
        ReturnTypeCapture::TypeExpr(t) => match scope.resolve_type_expr(&t, None) {
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

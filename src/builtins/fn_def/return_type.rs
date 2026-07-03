//! FN return-type pipeline: extraction → classification → carriage across the
//! dep-finish boundary → resolution at finish time.

use std::collections::HashMap;

use crate::machine::core::LexicalFrame;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::types::TypeResolution;
use crate::machine::model::types::{DeferredReturn, ReturnType};
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::{KError, KErrorKind, NodeId, Scope};
use crate::scheduler::DepResults;
use std::rc::Rc;

use super::param_refs::{kexpression_references_any, type_expr_references_any};

/// `ExprCarrier` is captured raw rather than sub-dispatched in the outer scope because a
/// `:(…)` / dotted return's inner expression may reference a parameter unbound there. It
/// arrives via the `:SigiledTypeExpr` return overload, whose `resolve_for` unwraps the
/// sigil to its inner `KObject::KExpression`.
pub(crate) enum ReturnTypeRaw<'a> {
    Resolved(KType<'a>),
    TypeExprCarrier(TypeIdentifier),
    ExprCarrier(KExpression<'a>),
}

/// `Deferred` skips the outer-scope elaborator entirely: running it would surface
/// `Unbound` because the referenced parameter is not in the FN's lexical scope.
/// Per-call elaboration runs at the dispatch boundary instead.
pub(crate) enum ReturnTypeState<'a> {
    Done(KType<'a>),
    Pending {
        te: TypeIdentifier,
        producers: Vec<NodeId>,
    },
    Deferred(DeferredReturn<'a>),
    /// `Expression(_)` carrier (e.g. `-> :(Mo.Ty)`) that doesn't reference any FN
    /// parameter; safe to resolve once at FN-def time. Scheduling happens via
    /// `super::finalize::defer` so all owned-sub registration lives
    /// at one site.
    ExprToSubDispatch(KExpression<'a>),
}

pub(crate) enum ReturnTypeCapture<'a> {
    Resolved(KType<'a>),
    Unresolved(String),
    Deferred(DeferredReturn<'a>),
    /// `owned_pos` is the return-type sub's index within the dep-finish's owned results — it is
    /// always the first owned dep, scheduled ahead of any signature subs, so `owned_pos == 0`.
    ReturnTypeExpr {
        owned_pos: usize,
    },
}

/// Read the `return_type` slot from a `BodyCtx::args` record. A `Type`-arm `KType` (bare-leaf
/// `Unresolved` → `TypeExprCarrier`, else `Resolved`), or an `Object`-arm `KObject::KExpression`
/// (`:(…)` / dotted return → `ExprCarrier`).
pub(crate) fn extract_return_type_raw<'a>(args: &KObject<'a>) -> Result<ReturnTypeRaw<'a>, KError> {
    use crate::machine::core::kfunction::action::{arg_object, arg_type};
    if let Some(kt) = arg_type(args, "return_type") {
        match kt {
            KType::Unresolved(te) => Ok(ReturnTypeRaw::TypeExprCarrier(te.clone())),
            t => Ok(ReturnTypeRaw::Resolved(t.clone())),
        }
    } else if let Some(KObject::KExpression(e)) = arg_object(args, "return_type") {
        Ok(ReturnTypeRaw::ExprCarrier(e.clone()))
    } else {
        Err(KError::new(KErrorKind::ShapeError(
            "FN return-type slot must be a type expression (e.g. `Number`, `:(LIST OF Str)`)"
                .to_string(),
        )))
    }
}

/// FUNCTOR-return admissibility verdict. FN paths pass `functor_param_types: None` and
/// ignore the verdict; FUNCTOR paths pass the param-name → declared-`KType` map so the
/// deferred-arm verdict resolves in the same walk.
pub(crate) enum AdmissibleVerdict {
    Admissible,
    /// `Pending` and `ExprToSubDispatch` carriers can't be classified until the resolved
    /// `KType` is in hand; the `is_functor: true` flag threaded through
    /// `defer` re-runs the predicate at dep-finish.
    Deferred,
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
    scope: &Scope<'a>,
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
                return Ok((ReturnTypeState::Deferred(DeferredReturn::Type(te)), verdict));
            }
            // Gated to the FN's lexical position — a return type naming a later type is a
            // position error, like any other forward reference.
            let state = match scope.resolve_type_identifier(&te, chain) {
                TypeResolution::Done(resolved) => ReturnTypeState::Done(resolved.kt.clone()),
                TypeResolution::Park(producers) => ReturnTypeState::Pending { te, producers },
                // `resolve_type_identifier` already tries the builtin fallback internally, so an
                // `Unbound` here is neither a type binder nor a builtin — a hard miss.
                TypeResolution::Unbound(msg) => {
                    return Err(KError::new(KErrorKind::ShapeError(format!(
                        "FN return-type slot: {msg}"
                    ))));
                }
            };
            let verdict = match &state {
                ReturnTypeState::Done(kt) => {
                    verdict_for_resolved(kt, functor_param_types.is_some())
                }
                _ => AdmissibleVerdict::Deferred,
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
                    AdmissibleVerdict::Deferred,
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
    te: &TypeIdentifier,
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

pub(super) fn make_capture<'a>(te: TypeIdentifier) -> ReturnTypeCapture<'a> {
    ReturnTypeCapture::Unresolved(te.render())
}

/// Park-arm outcomes from `Scope::resolve_type_identifier` are protocol errors here: every
/// parked producer is terminal by the dep-finish invariant, so a second park would
/// loop forever and is surfaced as a structured error.
pub(super) fn resolve_capture_at_finish<'a>(
    capture: ReturnTypeCapture<'a>,
    scope: &Scope<'a>,
    results: DepResults<'_, Carried<'a>>,
) -> Result<ReturnType<'a>, KError> {
    match capture {
        ReturnTypeCapture::Resolved(kt) => Ok(ReturnType::Resolved(kt)),
        ReturnTypeCapture::Unresolved(name) => {
            let te = TypeIdentifier::leaf(name.clone());
            match scope.resolve_type_identifier(&te, None) {
                TypeResolution::Done(resolved) => Ok(ReturnType::Resolved(resolved.kt.clone())),
                TypeResolution::Park(_) => Err(KError::new(KErrorKind::ShapeError(
                    "FN return type parked after dep-finish wake".to_string(),
                ))),
                // The builtin fallback is already tried inside `resolve_type_identifier`.
                TypeResolution::Unbound(msg) => Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN return-type slot: {msg}"
                )))),
            }
        }
        ReturnTypeCapture::Deferred(d) => Ok(ReturnType::Deferred(d)),
        ReturnTypeCapture::ReturnTypeExpr { owned_pos } => match *results.owned(owned_pos) {
            Carried::Type(kt) => Ok(ReturnType::Resolved(kt.clone())),
            Carried::Object(other) => Err(KError::new(KErrorKind::ShapeError(format!(
                "FN return-type slot sub-Dispatch expected a type expression, \
                 got a {} value",
                other.ktype().name(),
            )))),
        },
    }
}

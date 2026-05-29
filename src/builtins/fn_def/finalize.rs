//! Post-classification side of FN-def: turn the (return-type, parameter-list)
//! pair into either a synchronous `finalize_fn` call or a Combine-deferred
//! schedule, and own the Combine finish closure.
//!
//! [`classify`] collapses the 8-combinatoric `(ReturnTypeState × ParamListResult)`
//! decision tree to an [`FnPlan`] with two terminal shapes, so the caller in
//! `super::fn_def` reduces to a two-arm match.
//!
//! The FUNCTOR binder rides the same path with `is_functor: true` threaded
//! through. The flag flips the `KFunction::is_functor` carrier bit and gates
//! the FUNCTOR-only return-type admissibility check; no closure plumbing.

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{Elaborator, ReturnType};
use crate::machine::model::{ExpressionSignature, KObject, SignatureElement};
use crate::machine::{
    BindingIndex, Body, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope,
    SchedulerHandle,
};

use super::return_type::{
    make_capture, resolve_capture_at_finish, ReturnTypeCapture, ReturnTypeState,
};
use super::signature::{parse_fn_param_list, ParamListOutcome};

/// Local mirror of [`ParamListOutcome`] minus the structural-error variant
/// (short-circuited before [`classify`] runs) and with `Pending`'s payload
/// kept by-value so the planning match stays readable.
pub(crate) enum ParamListResult<'a> {
    Done(Vec<SignatureElement<'a>>),
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
}

/// Terminal shape of FN-def's planning step.
pub(crate) enum FnPlan<'a> {
    Synchronous {
        elements: Vec<SignatureElement<'a>>,
        return_type: ReturnType<'a>,
    },
    Combine(CombineInputs<'a>),
}

/// Inputs to [`defer_via_combine`]: carrier that survives the Combine boundary
/// plus the two parking lists.
pub(crate) struct CombineInputs<'a> {
    pub capture: ReturnTypeCapture<'a>,
    /// Existing sibling slots this Combine reads at finish-time but does NOT
    /// own. Installed as `Notify` (park) edges; must not cascade-free.
    pub park_producers: Vec<NodeId>,
    /// `Some` only when the return-type slot is an `Expression(_)` carrier that
    /// doesn't reference any FN parameter (resolves once at FN-def time, not
    /// per call). Scheduled ahead of `sub_dispatches` in the owned-sub region.
    pub return_type_sub: Option<KExpression<'a>>,
    /// `(slot_idx, sub_expr)` — `slot_idx` tells the finish closure which
    /// `signature_expr.parts` slot to splice the result into.
    pub sub_dispatches: Vec<(usize, KExpression<'a>)>,
}

/// Decide between the synchronous build path and the Combine-deferred path.
///
/// Arms differ only in how they shape the [`ReturnTypeCapture`] and merge the
/// two parking lists. All eight `(ReturnTypeState × ParamListResult)` combos
/// route to exactly one [`FnPlan`] outcome — no further routing downstream.
pub(crate) fn classify<'a>(
    rt: ReturnTypeState<'a>,
    params: ParamListResult<'a>,
) -> FnPlan<'a> {
    match (rt, params) {
        (ReturnTypeState::Done(kt), ParamListResult::Done(elements)) => FnPlan::Synchronous {
            elements,
            return_type: ReturnType::Resolved(kt),
        },
        (ReturnTypeState::Deferred(d), ParamListResult::Done(elements)) => FnPlan::Synchronous {
            elements,
            return_type: ReturnType::Deferred(d),
        },
        (ReturnTypeState::ExprToSubDispatch(e), ParamListResult::Done(_)) => {
            // Park empty, only the return-type sub: results[0] is its value.
            FnPlan::Combine(CombineInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { results_pos: 0 },
                park_producers: Vec::new(),
                return_type_sub: Some(e),
                sub_dispatches: Vec::new(),
            })
        }
        (
            ReturnTypeState::Done(kt),
            ParamListResult::Pending { park_producers, sub_dispatches },
        ) => FnPlan::Combine(CombineInputs {
            capture: ReturnTypeCapture::Resolved(kt),
            park_producers,
            return_type_sub: None,
            sub_dispatches,
        }),
        (
            ReturnTypeState::Deferred(d),
            ParamListResult::Pending { park_producers, sub_dispatches },
        ) => FnPlan::Combine(CombineInputs {
            // Return type is per-call-deferred: carry the carrier verbatim
            // through to `finalize_fn` once params land.
            capture: ReturnTypeCapture::Deferred(d),
            park_producers,
            return_type_sub: None,
            sub_dispatches,
        }),
        (
            ReturnTypeState::ExprToSubDispatch(e),
            ParamListResult::Pending { park_producers, sub_dispatches },
        ) => {
            // `[park ++ return_type_sub ++ sub_dispatches...]` puts the
            // return-type result at `results[park_producers.len()]`.
            let results_pos = park_producers.len();
            FnPlan::Combine(CombineInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { results_pos },
                park_producers,
                return_type_sub: Some(e),
                sub_dispatches,
            })
        }
        (ReturnTypeState::Pending { te, producers }, ParamListResult::Done(_)) => {
            // Synchronously elaborated `elements` are discarded; the wake
            // re-elaborates the param list against the spliced signature.
            FnPlan::Combine(CombineInputs {
                capture: make_capture(te),
                park_producers: producers,
                return_type_sub: None,
                sub_dispatches: Vec::new(),
            })
        }
        (
            ReturnTypeState::Pending { te, producers: rt_producers },
            ParamListResult::Pending { mut park_producers, sub_dispatches },
        ) => {
            park_producers.extend(rt_producers);
            FnPlan::Combine(CombineInputs {
                capture: make_capture(te),
                park_producers,
                return_type_sub: None,
                sub_dispatches,
            })
        }
    }
}

/// Build the `KFunction` and register it in `scope`. Shared between the
/// synchronous (no-park) path and the Combine-finish path.
pub(crate) fn finalize_fn<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement<'a>>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    finalize_fn_with_flag(scope, elements, return_type, body_expr, false, bind_index)
}

/// Variant used by both `finalize_fn` (FN: `is_functor=false`) and the FUNCTOR
/// builtin (`is_functor=true`).
///
/// When `is_functor` is true and `return_type` is `Resolved`, the carrier is
/// validated against [`KType::is_admissible_functor_return`] before the
/// `KFunction` is registered. `Deferred` carriers ride the surface-form check
/// at the FUNCTOR-binder site; the per-call dispatch boundary's `matches_value`
/// path catches any deferred carrier that resolves non-admissibly later.
pub(crate) fn finalize_fn_with_flag<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement<'a>>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    is_functor: bool,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    // FUNCTOR-only post-resolution return-type validation: fires here when the
    // return slot resolved at Combine-finish time rather than synchronously.
    if is_functor {
        if let ReturnType::Resolved(kt) = &return_type {
            if !kt.is_admissible_functor_return() {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTOR return-type slot must denote a module, signature, or functor; \
                     got `{}`",
                    kt.name(),
                ))));
            }
        }
    }
    // First Keyword keys the data table. Dispatch is by full signature via
    // `Bindings::functions`; `Bindings::data` is for discoverability /
    // shadow-by-name, neither of which has a single right answer for a
    // multi-token signature like `(a ADD b)`.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });
    let name = match name {
        Some(n) => n,
        None => {
            return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                    .to_string(),
            )));
        }
    };

    let user_sig = ExpressionSignature { return_type, elements };

    let arena = scope.arena;
    // `is_nominal_binder = false` regardless of `is_functor`: the FUNCTOR
    // carve-out is on the *binder builtin* (the FUNCTOR keyword), not on the
    // function it produces.
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_binder_and_functor(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
        None,
        None,
        is_functor,
        false,
    ));
    // `frame: None` — the scheduler's lift-on-return populates the Rc if this
    // KFunction value escapes a per-call body; top-level FNs have no frame.
    let obj: &'a KObject<'a> = arena.alloc(KObject::KFunction(f, None));
    if let Err(e) = scope.register_function(name, f, obj, bind_index) {
        return BodyResult::Err(e);
    }
    // Return the function reference so `LET f = (FN ...)` captures a callable
    // handle for the identifier-bound dispatch fallback.
    BodyResult::Value(obj)
}

/// Schedule a `Combine` over `park_producers` plus any newly scheduled
/// sub-Dispatches for parens-wrapped parameter types, then re-run the
/// signature elaboration in the finish closure.
///
/// Splice protocol: each entry in `inputs.sub_dispatches` is scheduled via
/// `sched.add_dispatch`; the resulting `NodeId` is appended to the Combine's
/// `deps` after the park producers. The finish closure splices each result
/// into `signature_expr.parts[slot_idx]` as `Future(obj)` before re-running
/// `parse_fn_param_list` against the now-final scope.
pub(crate) fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: KExpression<'a>,
    inputs: CombineInputs<'a>,
    body_expr: KExpression<'a>,
    is_functor: bool,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let CombineInputs { capture, park_producers, return_type_sub, sub_dispatches } = inputs;
    // Result layout: `[park_producers ++ return_type_sub? ++ sub_dispatches...]`.
    // Park producers are read-only (no cascade-free); the rest are owned subs.
    // `splice_layout[k] = (slot_idx, results_pos)` indexes the combined slice;
    // the return-type result is keyed separately by
    // `ReturnTypeCapture::ReturnTypeExpr { results_pos }` (set in `classify`).
    let park_count = park_producers.len();
    let mut owned_subs: Vec<NodeId> =
        Vec::with_capacity(return_type_sub.is_some() as usize + sub_dispatches.len());
    if let Some(rt_expr) = return_type_sub {
        owned_subs.push(sched.add_dispatch(rt_expr, scope));
    }
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, park_count + owned_subs.len()));
        owned_subs.push(id);
    }

    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let obj = results[results_pos];
            // Catch non-type results here so we can name the slot's part-index;
            // `parse_fn_param_list` would otherwise reject in its `Future(other)`
            // arm without that context.
            if !matches!(obj, KObject::KTypeValue(_)) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN signature slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    obj.ktype().name(),
                ))));
            }
            spliced_parts[slot_idx].value = ExpressionPart::Future(obj);
        }
        let spliced_signature = KExpression::new(spliced_parts);

        // Park producers have finalized — re-elaborate against the stable scope.
        // The elaborator's `Park` arm cannot fire again (every parked producer
        // is terminal by Combine-finish invariant); [`resolve_capture_at_finish`]
        // surfaces it as a structured error if it does.
        let mut elaborator = Elaborator::new(scope);
        let return_type: ReturnType<'a> = match resolve_capture_at_finish(capture, scope, results) {
            Ok(rt) => rt,
            Err(e) => return BodyResult::Err(e),
        };
        let elements = match parse_fn_param_list(&spliced_signature, &mut elaborator) {
            ParamListOutcome::Done(es) => es,
            ParamListOutcome::Err(msg) => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg)));
            }
            ParamListOutcome::Pending { .. } => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "FN signature elaboration still pending after Combine wake".to_string(),
                )));
            }
        };
        finalize_fn_with_flag(
            scope,
            elements,
            return_type,
            body_expr.clone(),
            is_functor,
            bind_index,
        )
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

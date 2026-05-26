//! Post‑classification side of FN‑def: turn the (return‑type, parameter‑list)
//! pair into either a synchronous `finalize_fn` call or a Combine‑deferred
//! schedule, and own the Combine finish closure.
//!
//! [`classify`] is the single point where the 8‑combinatoric `(ReturnTypeState ×
//! ParamListResult)` decision tree lives. It returns an [`FnPlan`] that names
//! the two terminal shapes: a synchronous build with fully‑resolved inputs, or
//! a [`CombineInputs`] bundle for the deferred path. `body` in `super::fn_def`
//! reduces to a two‑arm match on the plan.
//!
//! Park‑list merging — the bit that made the old match arms differ — lives
//! inside `classify` as small `park_producers.push(id)` / `.extend(_)` calls
//! against the produced `CombineInputs`. Everything downstream of `classify` is
//! shape‑uniform.
//!
//! The FUNCTOR binder rides this same path with `is_functor: true` threaded
//! through [`finalize_fn_with_flag`] and [`defer_via_combine`]. The flag flips
//! the `KFunction::is_functor` carrier bit (so `function_value_ktype` projects
//! to `KType::KFunctor`) AND triggers the FUNCTOR-only return-type
//! admissibility check on the resolved carrier via
//! [`KType::is_admissible_functor_return`]. There is no closure / Box<dyn Fn>
//! plumbing — the FUNCTOR-specific rule reduces to a single predicate call on
//! the resolved `KType`, gated by the flag the caller already passes.

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{Elaborator, ReturnType};
use crate::machine::model::{ExpressionSignature, KObject, SignatureElement};
use crate::machine::{
    Body, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope, SchedulerHandle,
};

use super::return_type::{
    make_capture, resolve_capture_at_finish, ReturnTypeCapture, ReturnTypeState,
};
use super::signature::{parse_fn_param_list, ParamListOutcome};

/// Local mirror of [`ParamListOutcome`] minus the structural‑error variant
/// (short‑circuited in `body` before [`classify`] runs) and with `Pending`'s
/// payload kept by‑value so the planning `match` stays readable.
pub(crate) enum ParamListResult<'a> {
    Done(Vec<SignatureElement<'a>>),
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
}

/// The terminal shape of FN‑def's planning step. `Synchronous` carries inputs
/// to [`finalize_fn`] directly; `Combine` carries a [`CombineInputs`] bundle
/// that [`defer_via_combine`] schedules.
pub(crate) enum FnPlan<'a> {
    Synchronous {
        elements: Vec<SignatureElement<'a>>,
        return_type: ReturnType<'a>,
    },
    Combine(CombineInputs<'a>),
}

/// Inputs to [`defer_via_combine`]: the carrier that survives the Combine
/// boundary plus the two parking lists (placeholder producers and pending
/// sub‑Dispatches). Held by value; consumed by the deferred path.
pub(crate) struct CombineInputs<'a> {
    pub capture: ReturnTypeCapture<'a>,
    /// Existing sibling slots whose values this Combine reads at finish-time
    /// but does NOT own. Install as `Notify` (park) edges.
    pub park_producers: Vec<NodeId>,
    /// Return-type expression to sub-Dispatch when this Combine is scheduled.
    /// `Some` only when the return-type slot is an `Expression(_)` carrier that
    /// doesn't reference any FN parameter (resolves once at FN-def time, not
    /// per call). Scheduled by [`defer_via_combine`] into the owned-sub region
    /// of the Combine layout, ahead of `sub_dispatches`.
    pub return_type_sub: Option<KExpression<'a>>,
    /// Param-type expressions to sub-Dispatch when this Combine is scheduled.
    /// Each `(slot_idx, sub_expr)` becomes an owned sub; the slot_idx tells the
    /// finish closure which `signature_expr.parts` slot to splice the result
    /// into.
    pub sub_dispatches: Vec<(usize, KExpression<'a>)>,
}

/// Decide between the synchronous build path and the Combine‑deferred path.
///
/// The arms differ only in how they shape the [`ReturnTypeCapture`] and how
/// they merge the two parking lists (return‑type sub‑Dispatch + parameter‑list
/// parking). All eight `(ReturnTypeState × ParamListResult)` combos route to
/// exactly one [`FnPlan`] outcome — no further routing downstream.
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
            // Return-type sub-Dispatch (owned by this Combine), params synchronous.
            // Combine layout: `[park ++ return_type_sub? ++ sub_dispatches...]`.
            // With park empty and only the return-type sub, results[0] is its value.
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
            // Params still parking on outer placeholders, but the return type is
            // per-call-deferred and doesn't need re-elaboration at the Combine wake.
            // Carry the carrier verbatim through to `finalize_fn` once params land.
            capture: ReturnTypeCapture::Deferred(d),
            park_producers,
            return_type_sub: None,
            sub_dispatches,
        }),
        (
            ReturnTypeState::ExprToSubDispatch(e),
            ParamListResult::Pending { park_producers, sub_dispatches },
        ) => {
            // Mixed shape: return-type sub-Dispatch (owned) joins the Combine alongside
            // parking parameter-types (park) and parameter-type sub-Dispatches (owned).
            // Combine layout `[park ++ return_type_sub ++ sub_dispatches...]` puts the
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
            // Param-types fully elaborated synchronously, but the return type parked.
            // The Combine finish re-runs both walks against the now-final scope for
            // symmetry. Synchronously elaborated `elements` are discarded; the wake
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
/// synchronous (no‑park) path and the Combine‑finish path. `is_functor`
/// flips the FUNCTOR-binder seam (set by the FUNCTOR builtin; FN always
/// passes `false`).
pub(crate) fn finalize_fn<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement<'a>>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
) -> BodyResult<'a> {
    finalize_fn_with_flag(scope, elements, return_type, body_expr, false)
}

/// Underlying variant used by both `finalize_fn` (FN: `is_functor=false`) and
/// the FUNCTOR builtin (`is_functor=true`). Kept separate so the existing
/// `finalize_fn` shape stays a thin façade and the FUNCTOR-binder change site
/// is the only place where the flag's truth-value choice surfaces.
///
/// When `is_functor` is true and `return_type` is `Resolved`, the carrier is
/// validated against [`KType::is_admissible_functor_return`] before the
/// `KFunction` is registered. `Deferred` carriers ride the surface-form check
/// that runs at the FUNCTOR-binder site (see `functor_def::body`); the
/// per-call dispatch boundary's `matches_value` path is the safety net for
/// any deferred carrier that resolves to a non-admissible shape later.
pub(crate) fn finalize_fn_with_flag<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement<'a>>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    is_functor: bool,
) -> BodyResult<'a> {
    // FUNCTOR-only post-resolution return-type validation. Mirror of the
    // synchronous-arm check in `functor_def::body` — fires here when the
    // return slot rode the Combine path through `Pending` / `ExprToSubDispatch`
    // and only resolved at Combine-finish time. The diagnostic still surfaces
    // at the FUNCTOR site (this is the Combine the FUNCTOR builtin scheduled).
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
    // Pick the first Keyword as the data-table key. `Bindings::functions` does the
    // load-bearing dispatch lookup by signature; `Bindings::data` is mostly for
    // discoverability and shadow-by-name semantics, neither of which has a single
    // right answer for a multi-token signature like `(a ADD b)`. First Keyword is
    // a defensible default.
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
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_pre_run_and_functor(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
        None,
        is_functor,
    ));
    // `frame: None` here — the lift-on-return logic in the scheduler will populate
    // the Rc when this KFunction value escapes out of a per-call body. For top-level
    // FNs, there's no per-call frame to clone, so None stays.
    let obj: &'a KObject<'a> = arena.alloc(KObject::KFunction(f, None));
    if let Err(e) = scope.register_function(name, f, obj) {
        return BodyResult::Err(e);
    }
    // Returning the function reference (rather than null) lets callers do
    // `LET f = (FN ...)` to capture a callable handle, which the dispatch fallback
    // for identifier-bound KFunctions can then invoke.
    BodyResult::Value(obj)
}

/// Schedule a `Combine` over `park_producers` plus any newly scheduled
/// sub‑Dispatches for parens‑wrapped parameter types, then re‑run the
/// signature elaboration in the finish closure. Mirrors MODULE / SIG's
/// `BodyResult::DeferTo` shape: the FN's terminal lifts off the Combine's
/// terminal, so the parent scope's binding lands at Combine‑finish time.
///
/// Splice protocol: every entry in `inputs.sub_dispatches` is scheduled here as
/// `sched.add_dispatch(sub_expr, scope)`; the resulting `NodeId` is appended to
/// the Combine's `deps` vector after the park producers. The closure tracks
/// each sub‑Dispatch's `(slot_idx, results_pos)` pairing so that when the
/// Combine wakes, the finish closure splices each result into
/// `signature_expr.parts[slot_idx]` as `Future(obj)` before re‑running
/// `parse_fn_param_list` against the now‑final scope.
///
/// `is_functor` is threaded through to [`finalize_fn_with_flag`] in the
/// finish closure: FN's `body` passes `false`; the FUNCTOR builtin passes
/// `true`. The flag is the only FUNCTOR-specific shape this function sees —
/// the post-Combine return-type admissibility check lives at the
/// `finalize_fn_with_flag` seam (no closure plumbing through this function).
pub(crate) fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: KExpression<'a>,
    inputs: CombineInputs<'a>,
    body_expr: KExpression<'a>,
    is_functor: bool,
) -> BodyResult<'a> {
    let CombineInputs { capture, park_producers, return_type_sub, sub_dispatches } = inputs;
    // Combine result layout: `[park_producers ++ return_type_sub? ++ sub_dispatches...]`.
    // `park_producers` are existing sibling slots (typically top-level SIG /
    // LET dispatches) the Combine reads but does NOT own — they must NOT be
    // cascade-freed at success. `return_type_sub` (when Some) and
    // `sub_dispatches` both become owned sub-Dispatches scheduled here and
    // cascade-freed at success. `splice_layout[k] = (slot_idx, results_pos)`
    // says "splice results[results_pos] into signature.parts[slot_idx] as
    // `Future(_)`"; `results_pos` indexes the combined `[park ++ owned ...]`
    // slice. The return-type result has no signature slot to splice into —
    // `ReturnTypeCapture::ReturnTypeExpr { results_pos }` carries its index
    // separately (set in `classify` to match this layout).
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
        // Splice each sub-Dispatch's result into the corresponding signature slot
        // as a `Future(_)`. Cloning `signature_expr` keeps the closure callable on
        // a hypothetical future re-wake (the Combine fires once today, but the
        // `KExpression` clone is cheap and matches the pattern used for `body_expr`).
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let obj = results[results_pos];
            // Reject non-type results early with a focused diagnostic. The downstream
            // `parse_fn_param_list` would also reject (its `Future(other)` arm), but
            // catching here lets us name the offending slot's part-index in the message.
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

        // Park producers have finalized — re-elaborate against the now-stable scope.
        // The elaborator's `Park` arm cannot fire again because every parked producer
        // is terminal by the Combine-finish invariant; if it does,
        // [`resolve_capture_at_finish`] surfaces it as a structured error rather
        // than re-parking forever.
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
        // FUNCTOR-only return-type validation runs inside `finalize_fn_with_flag`
        // when `is_functor: true`. No closure or `Box<dyn Fn>` plumbing here —
        // the flag the caller already threads through is the only signal needed.
        finalize_fn_with_flag(scope, elements, return_type, body_expr.clone(), is_functor)
    });
    let combine_id = sched.add_combine(owned_subs, park_producers, scope, finish);
    BodyResult::DeferTo(combine_id)
}

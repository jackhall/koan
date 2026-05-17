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
pub(super) enum ParamListResult<'a> {
    Done(Vec<SignatureElement>),
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
}

/// The terminal shape of FN‑def's planning step. `Synchronous` carries inputs
/// to [`finalize_fn`] directly; `Combine` carries a [`CombineInputs`] bundle
/// that [`defer_via_combine`] schedules.
pub(super) enum FnPlan<'a> {
    Synchronous {
        elements: Vec<SignatureElement>,
        return_type: ReturnType<'a>,
    },
    Combine(CombineInputs<'a>),
}

/// Inputs to [`defer_via_combine`]: the carrier that survives the Combine
/// boundary plus the two parking lists (placeholder producers and pending
/// sub‑Dispatches). Held by value; consumed by the deferred path.
pub(super) struct CombineInputs<'a> {
    pub capture: ReturnTypeCapture<'a>,
    pub park_producers: Vec<NodeId>,
    pub sub_dispatches: Vec<(usize, KExpression<'a>)>,
}

/// Decide between the synchronous build path and the Combine‑deferred path.
///
/// The arms differ only in how they shape the [`ReturnTypeCapture`] and how
/// they merge the two parking lists (return‑type sub‑Dispatch + parameter‑list
/// parking). All eight `(ReturnTypeState × ParamListResult)` combos route to
/// exactly one [`FnPlan`] outcome — no further routing downstream.
pub(super) fn classify<'a>(
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
        (ReturnTypeState::ExprSubDispatched(id), ParamListResult::Done(_)) => {
            // Return-type sub-dispatched, params synchronous. The Combine's only
            // dep is the return-type sub-Dispatch; the closure reads `results[0]`.
            FnPlan::Combine(CombineInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { results_pos: 0 },
                park_producers: vec![id],
                sub_dispatches: Vec::new(),
            })
        }
        (
            ReturnTypeState::Done(kt),
            ParamListResult::Pending { park_producers, sub_dispatches },
        ) => FnPlan::Combine(CombineInputs {
            capture: ReturnTypeCapture::Resolved(kt),
            park_producers,
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
            sub_dispatches,
        }),
        (
            ReturnTypeState::ExprSubDispatched(id),
            ParamListResult::Pending { mut park_producers, sub_dispatches },
        ) => {
            // Mixed shape: return-type sub-dispatch joins the Combine alongside any
            // parking parameter-types and parameter-type sub-Dispatches. Append the
            // return-type id to `park_producers` first; its `results_pos` is the
            // pre-push length (i.e. the next slot). The closure reads
            // `results[results_pos]` exactly there.
            let results_pos = park_producers.len();
            park_producers.push(id);
            FnPlan::Combine(CombineInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { results_pos },
                park_producers,
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
                sub_dispatches,
            })
        }
    }
}

/// Build the `KFunction` and register it in `scope`. Shared between the
/// synchronous (no‑park) path and the Combine‑finish path.
pub(super) fn finalize_fn<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
) -> BodyResult<'a> {
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
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::new(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
    ));
    // `frame: None` here — the lift-on-return logic in the scheduler will populate
    // the Rc when this KFunction value escapes out of a per-call body. For top-level
    // FNs, there's no per-call frame to clone, so None stays.
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
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
pub(super) fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: KExpression<'a>,
    inputs: CombineInputs<'a>,
    body_expr: KExpression<'a>,
) -> BodyResult<'a> {
    let CombineInputs { capture, park_producers, sub_dispatches } = inputs;
    // Schedule sub-Dispatches up front. `splice_layout[k] = (slot_idx, results_pos)`
    // says "splice results[results_pos] into signature.parts[slot_idx] as
    // `Future(_)`". `results_pos` is captured as `deps.len()` immediately before
    // the new dep is pushed, so the offset over `park_producers` falls out
    // naturally — Combine's `results` slice mirrors `deps` order, park producers
    // first.
    let mut deps: Vec<NodeId> = park_producers;
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, deps.len()));
        deps.push(id);
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
            spliced_parts[slot_idx] = ExpressionPart::Future(obj);
        }
        let spliced_signature = KExpression { parts: spliced_parts };

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
        finalize_fn(scope, elements, return_type, body_expr.clone())
    });
    let combine_id = sched.add_combine(deps, scope, finish);
    BodyResult::DeferTo(combine_id)
}

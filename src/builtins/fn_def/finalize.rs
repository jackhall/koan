//! Post-classification side of FN-def: turn the (return-type, parameter-list)
//! pair into either a synchronous `finalize_fn_with_kind` call or a Combine-deferred
//! schedule, and own the Combine finish closure.
//!
//! [`classify`] collapses the 8-combinatoric `(ReturnTypeState × ParamListResult)`
//! decision tree to an [`FnPlan`] with two terminal shapes, so the caller in
//! `super::fn_def` reduces to a two-arm match.
//!
//! The FUNCTOR and anonymous-FN binders ride the same path, selected by the
//! [`FnKind`] threaded through `finalize_fn_with_kind` / `defer_via_combine`:
//! `Functor` flips the `KFunction::is_functor` carrier bit and gates the
//! FUNCTOR-only return-type admissibility check; `Anonymous` (the `FN :{…}`
//! record-schema binder) skips registration. No closure plumbing.

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{Elaborator, ReturnType};
use crate::machine::model::Carried;
use crate::machine::model::{ExpressionSignature, KObject, SignatureElement};
use crate::machine::{
    BindingIndex, Body, BodyResult, CombineFinish, KError, KErrorKind, NodeId, SchedulerHandle,
    Scope,
};

use super::return_type::{
    make_capture, resolve_capture_at_finish, ReturnTypeCapture, ReturnTypeState,
};
use super::signature::{parse_fn_param_list, ParamListOutcome};

/// How a finalized FN-def is wired into the scope:
///
/// - `Function` — a keyworded FN registers under its lead keyword.
/// - `Functor` — same registration; additionally flips the `is_functor` carrier
///   bit and gates the FUNCTOR return-type admissibility check.
/// - `Anonymous` — a record-schema binder (`FN :{…}`) has no keyword, so it
///   registers nothing; the value it evaluates to is its only handle.
#[derive(Clone, Copy)]
pub(crate) enum FnKind {
    Function,
    Functor,
    Anonymous,
}

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
    /// `Some` for the anonymous (`FN :{…}`) path: the parameter list is already
    /// built from the resolved record schema, so the finish closure uses it
    /// verbatim instead of re-parsing `signature_expr` (which the anonymous path
    /// has no keyword/arg form of). `None` for the keyworded FN / FUNCTOR paths,
    /// which re-elaborate the spliced signature.
    pub prebuilt_elements: Option<Vec<SignatureElement<'a>>>,
}

/// Decide between the synchronous build path and the Combine-deferred path.
///
/// Arms differ only in how they shape the [`ReturnTypeCapture`] and merge the
/// two parking lists. All eight `(ReturnTypeState × ParamListResult)` combos
/// route to exactly one [`FnPlan`] outcome — no further routing downstream.
pub(crate) fn classify<'a>(rt: ReturnTypeState<'a>, params: ParamListResult<'a>) -> FnPlan<'a> {
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
                prebuilt_elements: None,
            })
        }
        (
            ReturnTypeState::Done(kt),
            ParamListResult::Pending {
                park_producers,
                sub_dispatches,
            },
        ) => FnPlan::Combine(CombineInputs {
            capture: ReturnTypeCapture::Resolved(kt),
            park_producers,
            return_type_sub: None,
            sub_dispatches,
            prebuilt_elements: None,
        }),
        (
            ReturnTypeState::Deferred(d),
            ParamListResult::Pending {
                park_producers,
                sub_dispatches,
            },
        ) => FnPlan::Combine(CombineInputs {
            // Return type is per-call-deferred: carry the carrier verbatim
            // through to `finalize_fn_with_kind` once params land.
            capture: ReturnTypeCapture::Deferred(d),
            park_producers,
            return_type_sub: None,
            sub_dispatches,
            prebuilt_elements: None,
        }),
        (
            ReturnTypeState::ExprToSubDispatch(e),
            ParamListResult::Pending {
                park_producers,
                sub_dispatches,
            },
        ) => {
            // `[park ++ return_type_sub ++ sub_dispatches...]` puts the
            // return-type result at `results[park_producers.len()]`.
            let results_pos = park_producers.len();
            FnPlan::Combine(CombineInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { results_pos },
                park_producers,
                return_type_sub: Some(e),
                sub_dispatches,
                prebuilt_elements: None,
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
                prebuilt_elements: None,
            })
        }
        (
            ReturnTypeState::Pending {
                te,
                producers: rt_producers,
            },
            ParamListResult::Pending {
                mut park_producers,
                sub_dispatches,
            },
        ) => {
            park_producers.extend(rt_producers);
            FnPlan::Combine(CombineInputs {
                capture: make_capture(te),
                park_producers,
                return_type_sub: None,
                sub_dispatches,
                prebuilt_elements: None,
            })
        }
    }
}

/// Variant used by the keyworded FN (`FnKind::Function`), the FUNCTOR builtin
/// (`FnKind::Functor`), and the anonymous record-schema binder
/// (`FnKind::Anonymous`).
///
/// `Functor` additionally validates a `Resolved` return type against
/// [`KType::is_admissible_functor_return`] before the `KFunction` is registered;
/// `Deferred` carriers ride the surface-form check at the FUNCTOR-binder site,
/// and the per-call dispatch boundary's `matches_value` path catches any
/// deferred carrier that resolves non-admissibly later. `Anonymous` skips
/// registration entirely — the value it returns is the function's only handle.
pub(crate) fn finalize_fn_with_kind<'a>(
    scope: &Scope<'a>,
    elements: Vec<SignatureElement<'a>>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    kind: FnKind,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let is_functor = matches!(kind, FnKind::Functor);
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
    // multi-token signature like `(a ADD b)`. An anonymous FN has no keyword,
    // so `name` is `None` and registration is skipped below.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });

    let user_sig = ExpressionSignature {
        return_type,
        elements,
    };

    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::with_binder_and_functor(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
        None,
        None,
        is_functor,
    ));
    // `frame: None` — the scheduler's lift-on-return populates the Rc if this
    // KFunction value escapes a per-call body; top-level FNs have no frame.
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    // An anonymous FN registers nothing — its only handle is the returned value
    // (LET-bound or dropped into a function-typed slot). A keyworded FN / FUNCTOR
    // registers under its lead keyword.
    if !matches!(kind, FnKind::Anonymous) {
        let name = match name {
            Some(n) => n,
            None => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                        .to_string(),
                )));
            }
        };
        if let Err(e) = scope.register_function(name, f, obj, bind_index) {
            return BodyResult::Err(e);
        }
    }
    // Return the function reference so `LET f = (FN ...)` captures a callable
    // handle for the identifier-bound dispatch fallback.
    BodyResult::value(obj)
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
pub(crate) fn defer_via_combine<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    signature_expr: KExpression<'a>,
    inputs: CombineInputs<'a>,
    body_expr: KExpression<'a>,
    kind: FnKind,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let CombineInputs {
        capture,
        park_producers,
        return_type_sub,
        sub_dispatches,
        prebuilt_elements,
    } = inputs;
    // Result layout: `[park_producers ++ return_type_sub? ++ sub_dispatches...]`.
    // Park producers are read-only (no cascade-free); the rest are owned subs.
    // `splice_layout[k] = (slot_idx, results_pos)` indexes the combined slice;
    // the return-type result is keyed separately by
    // `ReturnTypeCapture::ReturnTypeExpr { results_pos }` (set in `classify`).
    let park_count = park_producers.len();
    let mut owned_subs: Vec<NodeId> =
        Vec::with_capacity(return_type_sub.is_some() as usize + sub_dispatches.len());
    if let Some(rt_expr) = return_type_sub {
        owned_subs.push(sched.add_dispatch_here(rt_expr));
    }
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch_here(sub_expr);
        splice_layout.push((slot_idx, park_count + owned_subs.len()));
        owned_subs.push(id);
    }

    let finish: CombineFinish<'a> = Box::new(move |_sched, results| {
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let carrier = results[results_pos];
            // Catch non-type results here so we can name the slot's part-index;
            // `parse_fn_param_list` would otherwise reject in its `Future(other)`
            // arm without that context.
            if !matches!(carrier, Carried::Type(_)) {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN signature slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    carrier.ktype().name(),
                ))));
            }
            spliced_parts[slot_idx].value = ExpressionPart::Future(carrier);
        }
        let spliced_signature = KExpression::new(spliced_parts);

        // Park producers have finalized — resolve against the stable scope.
        // [`resolve_capture_at_finish`] surfaces a re-park as a structured error
        // (every parked producer is terminal by the Combine-finish invariant).
        let return_type: ReturnType<'a> =
            match resolve_capture_at_finish(capture, _sched.current_scope(), results) {
                Ok(rt) => rt,
                Err(e) => return BodyResult::Err(e),
            };
        // The anonymous (`FN :{…}`) path supplies its parameter list pre-built
        // from the resolved record schema; the keyworded FN / FUNCTOR path
        // re-elaborates the spliced signature.
        let elements = match prebuilt_elements {
            Some(es) => es,
            None => {
                let mut elaborator = Elaborator::new(_sched.current_scope());
                match parse_fn_param_list(&spliced_signature, &mut elaborator) {
                    ParamListOutcome::Done(es) => es,
                    ParamListOutcome::Err(msg) => {
                        return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg)));
                    }
                    ParamListOutcome::Pending { .. } => {
                        return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                            "FN signature elaboration still pending after Combine wake".to_string(),
                        )));
                    }
                }
            }
        };
        finalize_fn_with_kind(
            _sched.current_scope(),
            elements,
            return_type,
            body_expr.clone(),
            kind,
            bind_index,
        )
    });
    let combine_id = sched.add_combine_here(owned_subs, park_producers, finish);
    BodyResult::DeferTo(combine_id)
}

/// `Action`-harness twin of [`defer_via_combine`]: build the same Combine as an `Action` — park
/// producers become `Dep::Existing`, the optional return-type sub and the param-type subs become
/// `Dep::Dispatch { OwnScope }` (in that order, so the `[park ++ rt? ++ subs]` result layout the
/// `splice_layout` / `ReturnTypeExpr { results_pos }` indices assume holds), and the finish routes
/// the still-`BodyResult`-returning [`finalize_fn_with_kind`] through `body_result_to_action`.
#[cfg(feature = "action-harness")]
pub(crate) fn defer_via_combine_action<'a>(
    signature_expr: KExpression<'a>,
    inputs: CombineInputs<'a>,
    body_expr: KExpression<'a>,
    kind: FnKind,
    bind_index: BindingIndex,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        body_result_to_action, Action, Cont, Dep, DepPlacement,
    };
    let CombineInputs {
        capture,
        park_producers,
        return_type_sub,
        sub_dispatches,
        prebuilt_elements,
    } = inputs;
    let park_count = park_producers.len();
    let mut deps: Vec<Dep<'a>> = park_producers.iter().copied().map(Dep::Existing).collect();
    let mut owned_count = 0usize;
    if let Some(rt_expr) = return_type_sub {
        deps.push(Dep::Dispatch {
            expr: rt_expr,
            placement: DepPlacement::OwnScope,
        });
        owned_count += 1;
    }
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        deps.push(Dep::Dispatch {
            expr: sub_expr,
            placement: DepPlacement::OwnScope,
        });
        splice_layout.push((slot_idx, park_count + owned_count));
        owned_count += 1;
    }
    let finish: Cont<'a> = Box::new(move |fctx, results| {
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, results_pos) in &splice_layout {
            let carrier = results[results_pos];
            if !matches!(carrier, Carried::Type(_)) {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN signature slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    carrier.ktype().name(),
                )))));
            }
            spliced_parts[slot_idx].value = ExpressionPart::Future(carrier);
        }
        let spliced_signature = KExpression::new(spliced_parts);
        let return_type: ReturnType<'a> =
            match resolve_capture_at_finish(capture, fctx.scope, results) {
                Ok(rt) => rt,
                Err(e) => return Action::Done(Err(e)),
            };
        let elements = match prebuilt_elements {
            Some(es) => es,
            None => {
                let mut elaborator = Elaborator::new(fctx.scope);
                match parse_fn_param_list(&spliced_signature, &mut elaborator) {
                    ParamListOutcome::Done(es) => es,
                    ParamListOutcome::Err(msg) => {
                        return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg))))
                    }
                    ParamListOutcome::Pending { .. } => {
                        return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                            "FN signature elaboration still pending after Combine wake".to_string(),
                        ))))
                    }
                }
            }
        };
        body_result_to_action(finalize_fn_with_kind(
            fctx.scope,
            elements,
            return_type,
            body_expr.clone(),
            kind,
            bind_index,
        ))
    });
    crate::machine::core::kfunction::action::Action::Combine { deps, finish }
}

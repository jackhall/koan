//! Post-classification side of FN-def: turn the (return-type, parameter-list)
//! pair into either a synchronous `finalize_fn_with_kind` call or a deferred
//! schedule, and own the dep-finish closure.
//!
//! [`classify`] collapses the 8-combinatoric `(ReturnTypeState × ParamListResult)`
//! decision tree to an [`FnPlan`] with two terminal shapes, so the caller in
//! `super::fn_def` reduces to a two-arm match.
//!
//! The keyworded and anonymous FN binders ride the same path, selected by the
//! [`FnKind`] threaded through `finalize_fn_with_kind` / `defer`.

use crate::machine::model::Carried;
use crate::machine::model::CarriedFamily;
use crate::machine::model::{Elaborator, ReturnType, TypeRegistry};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{ExpressionSignature, KObject, SignatureElement};
use crate::machine::Action;
use crate::machine::KFunction;
use crate::machine::StepCarried;
use crate::machine::{BindingIndex, Body, CarrierWitness, KError, KErrorKind, NodeId, Scope};
use crate::witnessed::Witnessed;

use super::return_type::{
    make_capture, resolve_capture_at_finish, ReturnTypeCapture, ReturnTypeState,
};
use super::signature::{parse_fn_param_list, ParamListOutcome};

/// How a finalized FN-def is wired into the scope:
///
/// - `Function` — a keyworded FN registers under its lead keyword.
/// - `Anonymous` — a record-schema binder (`FN :{…}`) has no keyword, so it
///   registers nothing; the value it evaluates to is its only handle.
#[derive(Clone, Copy)]
pub(crate) enum FnKind {
    Function,
    Anonymous,
}

/// Local mirror of [`ParamListOutcome`] minus the structural-error variant
/// (short-circuited before [`classify`] runs) and with `Pending`'s payload
/// kept by-value so the planning match stays readable.
pub(crate) enum ParamListResult<'a> {
    Done(Vec<SignatureElement>),
    Pending {
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<(usize, KExpression<'a>)>,
    },
}

/// Terminal shape of FN-def's planning step.
pub(crate) enum FnPlan<'a> {
    Synchronous {
        elements: Vec<SignatureElement>,
        return_type: ReturnType<'a>,
    },
    Deferred(DeferredInputs<'a>),
}

/// Inputs to [`defer`]: carrier that survives the dep-finish boundary
/// plus the two parking lists.
pub(crate) struct DeferredInputs<'a> {
    pub capture: ReturnTypeCapture<'a>,
    /// Existing sibling slots this dep-finish reads at finish-time but does NOT
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
    /// has no keyword/arg form of). `None` for the keyworded FN path, which
    /// re-elaborates the spliced signature.
    pub prebuilt_elements: Option<Vec<SignatureElement>>,
}

/// Decide between the synchronous build path and the deferred path.
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
            // Only the return-type sub, no parks: it is owned index 0.
            FnPlan::Deferred(DeferredInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { owned_pos: 0 },
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
        ) => FnPlan::Deferred(DeferredInputs {
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
        ) => FnPlan::Deferred(DeferredInputs {
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
            // The return-type sub is scheduled ahead of the signature subs, so it is owned index 0
            // regardless of how many producers are parked.
            FnPlan::Deferred(DeferredInputs {
                capture: ReturnTypeCapture::ReturnTypeExpr { owned_pos: 0 },
                park_producers,
                return_type_sub: Some(e),
                sub_dispatches,
                prebuilt_elements: None,
            })
        }
        (ReturnTypeState::Pending { te, producers }, ParamListResult::Done(_)) => {
            // Synchronously elaborated `elements` are discarded; the wake
            // re-elaborates the param list against the spliced signature.
            FnPlan::Deferred(DeferredInputs {
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
            FnPlan::Deferred(DeferredInputs {
                capture: make_capture(te),
                park_producers,
                return_type_sub: None,
                sub_dispatches,
                prebuilt_elements: None,
            })
        }
    }
}

/// Reject a bare type constructor in either of a function's value type positions. A parameter
/// annotation and a resolved return type each name the type of a value, so each must be a proper
/// type; a constructor of kind `* -> *` standing unapplied is a kind error. The single gate for
/// every FN surface — keyworded and anonymous, synchronous and dep-finished — since all of them
/// reach [`finalize_fn_with_kind`]. A [`ReturnType::Deferred`] carrier names a parameter and
/// elaborates per call, so it is checked at that boundary, not here.
fn check_value_type_kinds(
    elements: &[SignatureElement],
    return_type: &ReturnType<'_>,
    types: &TypeRegistry,
) -> Result<(), KError> {
    use crate::machine::model::unsaturated_constructor_message;
    for element in elements {
        if let SignatureElement::Argument(argument) = element {
            if let Some(message) = unsaturated_constructor_message(
                argument.ktype,
                &format!("the type of FN parameter `{}`", argument.name),
                types,
            ) {
                return Err(KError::new(KErrorKind::ShapeError(message)));
            }
        }
    }
    if let ReturnType::Resolved(kt) = return_type {
        if let Some(message) = unsaturated_constructor_message(*kt, "the FN return type", types) {
            return Err(KError::new(KErrorKind::ShapeError(message)));
        }
    }
    Ok(())
}

/// Build the `KFunction` and, for a keyworded `Function`, register it under its lead
/// keyword. `Anonymous` skips registration entirely — the value it returns is the
/// function's only handle.
pub(crate) fn finalize_fn_with_kind<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement>,
    return_type: ReturnType<'a>,
    body_expr: KExpression<'a>,
    kind: FnKind,
    bind_index: BindingIndex,
    types: &TypeRegistry,
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    check_value_type_kinds(&elements, &return_type, types)?;

    // First Keyword keys the data table. Dispatch is by full signature via
    // `Bindings::functions`; `Bindings::data` is for discoverability /
    // shadow-by-name, neither of which has a single right answer for a
    // multi-token signature like `(a ADD b)`.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });

    let user_sig = ExpressionSignature {
        return_type,
        elements,
    };

    let region = scope.brand();
    let f: &'a KFunction<'a> = region.alloc_function(KFunction::new(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
        None,
        None,
        types,
    ));
    // `frame: None` — the scheduler's lift-on-return populates the Rc if this
    // KFunction value escapes a per-call body; top-level FNs have no frame. `f` was just
    // allocated into `scope`'s own region above, so the checked audit always passes; the paired
    // token carries the home-borrow bit the audit walk derives (the captured `&Scope` into home).
    let (obj, stored) = scope
        .alloc_object_checked_stored(KObject::KFunction(f), types)
        .expect("f was just allocated into scope's own region");
    if !matches!(kind, FnKind::Anonymous) {
        let name = match name {
            Some(n) => n,
            None => {
                return Err(KError::new(KErrorKind::ShapeError(
                    "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                        .to_string(),
                )));
            }
        };
        scope.register_function(name, f, obj, bind_index)?;
    }
    // The FN value is co-located in its defining scope's region (owned signature / body, a `&Scope`
    // capture), and the captured scope — region-resident under that frame — transitively keeps every
    // foreign region its bindings reach alive through the scope's sealed reach-set. So a fresh FN
    // reaches nothing foreign (its captured scope is home or a home-pinned ancestor): its terminal
    // carrier is built with the empty foreign reach `stored` derived, witnessed by that scope's home
    // frame alone. `LET f = (FN ...)` still captures the callable via this carrier.
    Ok(scope.resident_value_carrier(obj, stored))
}

/// Wrap a [`finalize_fn_with_kind`] result in the action currency. The FN value is built witnessed
/// (it names its captured scope's frame), so success seals as [`Action::Done(Ok)`](Action::Done).
pub(crate) fn fn_action<'a>(
    result: Result<Witnessed<CarriedFamily, CarrierWitness>, KError>,
) -> Action<'a> {
    match result {
        Ok(witnessed) => Action::Done(Ok(StepCarried::born(witnessed))),
        Err(e) => Action::Done(Err(e)),
    }
}

/// Schedule an `AwaitDeps` over `park_producers` plus any newly scheduled
/// sub-Dispatches for parens-wrapped parameter types, then re-run the signature
/// elaboration in the finish closure.
///
/// Dep order is `[park ++ rt? ++ subs]`, so the owned indices `splice_layout` and
/// `ReturnTypeExpr` record stay stable regardless of how many producers are parked.
pub(crate) fn defer<'a>(
    signature_expr: KExpression<'a>,
    inputs: DeferredInputs<'a>,
    body_expr: KExpression<'a>,
    kind: FnKind,
    bind_index: BindingIndex,
) -> crate::machine::Action<'a> {
    use crate::machine::{Action, AwaitContinue, DepPlacement, DepRequest};
    let DeferredInputs {
        capture,
        park_producers,
        return_type_sub,
        sub_dispatches,
        prebuilt_elements,
    } = inputs;
    // `deps` is `[Existing parks..., Dispatch rt?, Dispatch subs...]`; the harness partitions it into
    // a `Deps` builder (parks first, owned in this order), so the return-type sub is owned index 0 and
    // the signature subs follow. `splice_layout` records each sub's owned index for the finish.
    let mut deps: Vec<DepRequest<'a>> = park_producers
        .iter()
        .copied()
        .map(DepRequest::Existing)
        .collect();
    let mut owned_count = 0usize;
    if let Some(rt_expr) = return_type_sub {
        deps.push(DepRequest::Dispatch {
            expr: rt_expr,
            placement: DepPlacement::OwnScope,
        });
        owned_count += 1;
    }
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        deps.push(DepRequest::Dispatch {
            expr: sub_expr,
            placement: DepPlacement::OwnScope,
        });
        splice_layout.push((slot_idx, owned_count));
        owned_count += 1;
    }
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let mut spliced_parts = signature_expr.parts.clone();
        for &(slot_idx, owned_pos) in &splice_layout {
            let terminal = results.owned(owned_pos);
            if !matches!(terminal.value, Carried::Type(_)) {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN signature slot at part-index {slot_idx} expected a type expression, \
                     got a {} value",
                    terminal.value.ktype(fctx.types).name(fctx.types),
                )))));
            }
            // The resolved type slot travels as its producer's own delivery envelope — carrier and
            // retained producer-frame owner as one unit — opened where the signature is assembled
            // (`parse_fn_param_list` adopts it through the elaborator's scope). The early-error check
            // above reads `terminal.value`, still delivered at the step brand; the envelope is the
            // survival, not a relocated copy, its host keeping the type's backing retained to the
            // adopting elaboration.
            spliced_parts[slot_idx].value = ExpressionPart::Spliced {
                cell: terminal.delivered.duplicate(),
            };
        }
        let spliced_signature = KExpression::new(spliced_parts);
        let return_type: ReturnType<'a> = crate::try_action!(resolve_capture_at_finish(
            capture, fctx.scope, results, fctx.types
        ));
        let elements = match prebuilt_elements {
            Some(es) => es,
            None => {
                let mut elaborator = Elaborator::new(fctx.scope);
                match parse_fn_param_list(&spliced_signature, &mut elaborator, fctx.types) {
                    ParamListOutcome::Done(es) => es,
                    ParamListOutcome::Err(msg) => {
                        return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg))))
                    }
                    ParamListOutcome::Pending { .. } => {
                        return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                            "FN signature elaboration still pending after dep-finish wake"
                                .to_string(),
                        ))))
                    }
                }
            }
        };
        fn_action(finalize_fn_with_kind(
            fctx.scope,
            elements,
            return_type,
            body_expr.clone(),
            kind,
            bind_index,
            fctx.types,
        ))
    });
    crate::machine::Action::AwaitDeps { deps, finish }
}

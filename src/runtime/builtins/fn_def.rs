mod signature;

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, Body, BodyResult, CombineFinish, KError, KErrorKind, KFunction, Scope, SchedulerHandle};
use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator};

use crate::runtime::machine::kfunction::argument_bundle::{
    extract_kexpression, extract_ktype, extract_type_name_ref,
};
use crate::ast::ExpressionPart;
use super::{err, register_builtin_with_pre_run};

use signature::ParamListOutcome;

pub(crate) use signature::pre_run;

/// `FN <signature:KExpression> -> <return_type:Type> = <body:KExpression>` — the user-defined
/// function constructor. The signature and body slots are `KType::KExpression`, so the parser's
/// parenthesized sub-expressions match and ride through as data. The return-type slot is a
/// `KType::TypeExprRef`, matching a parsed `Type(_)` token whose structured `TypeExpr` (with
/// any nested type parameters) is preserved through the bundle.
/// The captured signature `KExpression` is structurally inspected here — never dispatched —
/// to derive the registered function's `ExpressionSignature`. The body `KExpression` is
/// captured raw; `KFunction::invoke` substitutes parameter values into it and re-dispatches
/// at call time.
///
/// Signature shape: each `Keyword` part becomes a `SignatureElement::Keyword` (a fixed token
/// in the call site); each `Identifier` must be followed by `: Type` to form an `Argument`
/// triple, producing a typed parameter slot the caller supplies. Per-param types are
/// dispatch-checked via `Argument::matches`, so a call whose argument types don't satisfy
/// the signature surfaces as `DispatchFailed: no matching function` (same path as builtins);
/// overloads on different parameter types route to the right body via slot-specificity.
/// At least one `Keyword` is required so the signature has a fixed token to dispatch on —
/// a signature of all-Argument slots would shadow `value_lookup`/`value_pass`. Bare
/// identifiers (without `: Type`), stray type tokens, literals, and nested expressions in
/// the signature are rejected with a `ShapeError`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let signature_expr = match extract_kexpression(&mut bundle, "signature") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // The return-type slot holds either a fully-resolved `KTypeValue` (builtin leaf
    // names, structural shapes like `List<Number>`) or a `TypeNameRef` carrier (a bare
    // leaf name not in `KType::from_name`'s table — `Point`, `IntOrd`, `MyList`). Peek
    // first to pick the right extractor: both consume the slot via `remove`, so calling
    // one after the other on the same bundle would lose the value.
    enum ReturnTypeRaw {
        Resolved(KType),
        Carrier(crate::ast::TypeExpr),
    }
    let return_type_raw = match bundle.get("return_type") {
        Some(KObject::KTypeValue(_)) => match extract_ktype(&mut bundle, "return_type") {
            Some(t) => ReturnTypeRaw::Resolved(t),
            None => unreachable!("get(KTypeValue) then extract_ktype must succeed"),
        },
        Some(KObject::TypeNameRef(_, _)) => match extract_type_name_ref(&mut bundle, "return_type") {
            Some(te) => ReturnTypeRaw::Carrier(te),
            None => unreachable!("get(TypeNameRef) then extract_type_name_ref must succeed"),
        },
        _ => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN return-type slot must be a type expression (e.g. `Number`, `List<Str>`)"
                    .to_string(),
            )));
        }
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN body slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Multi-statement body desugar: a body whose parts are entirely sub-expressions
    // (`((a) (b) (c))`) right-folds into a chain of `CONS` calls so the scheduler sees a
    // single tail-callable expression. See [`super::cons`] for the contract — TCO holds
    // on the last statement, backward refs across statements work, forward refs do not.
    let body_expr = super::cons::fold_multi_statement(body_expr);

    // Phase-3 elaborator: parameter-type leaf names park on outstanding type-binding
    // placeholders, so a `LET MyList = (LIST_OF Number)` dispatched in the same batch
    // wakes the FN's signature elaboration when its body finalizes.
    let mut elaborator = Elaborator::new(scope);

    // Step 1: elaborate the return type. Three outcomes — concrete `KType`, parking on
    // producers (carries the original unresolved name so the Combine finish re-runs the
    // leaf elaboration), or a structured error.
    enum ReturnTypeState {
        Done(KType),
        Pending(String, Vec<crate::runtime::machine::NodeId>),
    }
    let return_type_state = match return_type_raw {
        ReturnTypeRaw::Resolved(kt) => ReturnTypeState::Done(kt),
        // Bare-leaf carrier path: walk the scope-aware elaborator against the parser-
        // preserved `TypeExpr`. The carrier's `name` survives bind regardless of
        // resolution outcome — the surface form is what diagnostics render and what
        // park-on-placeholder uses to wake later. Stage 2 keeps `ReturnTypeCapture`'s
        // `Unresolved(String)` carrier; the parser-preserved `TypeExpr` could carry
        // through if a future workload needs a parameterized user-typed return type,
        // but FN's shipped patterns are all bare leaves.
        ReturnTypeRaw::Carrier(te) => {
            let name = te.name.clone();
            match elaborate_type_expr(&mut elaborator, &te) {
                ElabResult::Done(kt) => ReturnTypeState::Done(kt),
                ElabResult::Park(producers) => ReturnTypeState::Pending(name, producers),
                ElabResult::Unbound(_) => match KType::from_name(&name) {
                    Some(kt) => ReturnTypeState::Done(kt),
                    None => {
                        return err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot: unknown type name `{name}`"
                        ))));
                    }
                },
            }
        }
    };

    // Step 2: elaborate the parameter list. Three sub-cases:
    //   * `Done(es)` — every slot resolved synchronously.
    //   * `Pending { park_producers, sub_dispatches }` — at least one slot needs a
    //     scheduler wake (placeholder finalization) or a sub-Dispatch (parens-wrapped
    //     type expression).
    //   * `Err(_)` — structural / unbound failure surfacing immediately.
    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

    // Step 3: route to synchronous-finalize or Combine-deferred based on parking state.
    match (return_type_state, params) {
        (ReturnTypeState::Done(rt), ParamListResult::Done(elements)) => {
            finalize_fn(scope, elements, rt, body_expr)
        }
        (ReturnTypeState::Done(rt), ParamListResult::Pending { park_producers, sub_dispatches }) => {
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Resolved(rt),
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
        (ReturnTypeState::Pending(name, producers), ParamListResult::Done(_)) => {
            // Param-types fully elaborated synchronously, but the return type parked. The
            // Combine finish re-runs both walks against the now-final scope for symmetry.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Unresolved(name),
                producers,
                Vec::new(),
                body_expr,
            )
        }
        (ReturnTypeState::Pending(name, rt_producers), ParamListResult::Pending { mut park_producers, sub_dispatches }) => {
            park_producers.extend(rt_producers);
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Unresolved(name),
                park_producers,
                sub_dispatches,
                body_expr,
            )
        }
    }
}

/// Local mirror of [`ParamListOutcome`] minus the structural-error variant (which is
/// short-circuited above) and with `Pending`'s payload kept by-value so the routing
/// `match` stays readable.
enum ParamListResult<'a> {
    Done(Vec<SignatureElement>),
    Pending {
        park_producers: Vec<crate::runtime::machine::NodeId>,
        sub_dispatches: Vec<(usize, crate::ast::KExpression<'a>)>,
    },
}

/// Carrier for the return type across the Combine boundary. `Resolved` means we already
/// have a concrete `KType` and the Combine finish skips re-elaboration; `Unresolved` means
/// we parked on the leaf name and the finish runs `elaborate_type_expr` against the
/// now-final scope.
enum ReturnTypeCapture {
    Resolved(KType),
    Unresolved(String),
}

/// Build the `KFunction` and register it in `scope`. Shared between the synchronous
/// (no-park) path and the Combine-finish path.
fn finalize_fn<'a>(
    scope: &'a Scope<'a>,
    elements: Vec<SignatureElement>,
    return_type: KType,
    body_expr: crate::ast::KExpression<'a>,
) -> BodyResult<'a> {
    // Pick the first Keyword as the data-table key. `Bindings::functions` does the load-
    // bearing dispatch lookup by signature; `Bindings::data` is mostly for discoverability
    // and shadow-by-name semantics, neither of which has a single right answer for a
    // multi-token signature like `(a ADD b)`. First Keyword is a defensible default.
    let name = elements.iter().find_map(|e| match e {
        SignatureElement::Keyword(s) => Some(s.clone()),
        _ => None,
    });
    let name = match name {
        Some(n) => n,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN signature must contain at least one Keyword (a fixed token to dispatch on)"
                    .to_string(),
            )));
        }
    };

    let user_sig = ExpressionSignature {
        return_type,
        elements,
    };

    let arena = scope.arena;
    let f: &'a KFunction<'a> = arena.alloc_function(KFunction::new(
        user_sig,
        Body::UserDefined(body_expr),
        scope,
    ));
    // `frame: None` here — the lift-on-return logic in the scheduler will populate the Rc
    // when this KFunction value escapes out of a per-call body. For top-level FNs, there's
    // no per-call frame to clone, so None stays.
    let obj: &'a KObject<'a> = arena.alloc_object(KObject::KFunction(f, None));
    if let Err(e) = scope.register_function(name, f, obj) {
        return err(e);
    }
    // Returning the function reference (rather than null) lets callers do
    // `LET f = (FN ...)` to capture a callable handle, which the dispatch fallback for
    // identifier-bound KFunctions can then invoke.
    BodyResult::Value(obj)
}

/// Schedule a `Combine` over `park_producers` plus any newly scheduled sub-Dispatches
/// for parens-wrapped parameter types, then re-run the signature elaboration in the
/// finish closure. Mirrors MODULE / SIG's `BodyResult::DeferTo` shape: the FN's terminal
/// lifts off the Combine's terminal, so the parent scope's binding lands at Combine-finish
/// time.
///
/// Splice protocol: every entry in `sub_dispatches` is scheduled here as
/// `sched.add_dispatch(sub_expr, scope)`; the resulting `NodeId` is appended to the
/// Combine's `deps` vector after the park producers. The closure tracks each
/// sub-Dispatch's `(slot_idx_in_signature_parts, position_in_results)` pairing so that
/// when the Combine wakes, the finish closure splices each result into
/// `signature_expr.parts[slot_idx]` as `Future(obj)` before re-running
/// `parse_fn_param_list` against the now-final scope.
fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: crate::ast::KExpression<'a>,
    return_type_capture: ReturnTypeCapture,
    park_producers: Vec<crate::runtime::machine::NodeId>,
    sub_dispatches: Vec<(usize, crate::ast::KExpression<'a>)>,
    body_expr: crate::ast::KExpression<'a>,
) -> BodyResult<'a> {
    // Schedule sub-Dispatches up front. `splice_layout[k] = (slot_idx, results_pos)` says
    // "splice results[results_pos] into signature.parts[slot_idx] as `Future(_)`".
    // `results_pos` is captured as `deps.len()` immediately before the new dep is pushed,
    // so the offset over `park_producers` falls out naturally — Combine's `results` slice
    // mirrors `deps` order, park producers first.
    let mut deps: Vec<crate::runtime::machine::NodeId> = park_producers;
    let mut splice_layout: Vec<(usize, usize)> = Vec::with_capacity(sub_dispatches.len());
    for (slot_idx, sub_expr) in sub_dispatches {
        let id = sched.add_dispatch(sub_expr, scope);
        splice_layout.push((slot_idx, deps.len()));
        deps.push(id);
    }

    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        // Splice each sub-Dispatch's result into the corresponding signature slot as a
        // `Future(_)`. Cloning the `signature_expr` keeps the closure callable on a
        // hypothetical future re-wake (the Combine fires once today, but the
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
        let spliced_signature = crate::ast::KExpression { parts: spliced_parts };

        // Park producers have finalized — re-elaborate against the now-stable scope. The
        // elaborator's `Park` arm cannot fire again because every parked producer is
        // terminal by the Combine-finish invariant; if it does, that's a regression
        // worth surfacing as a structured error rather than re-parking forever.
        let mut elaborator = Elaborator::new(scope);
        let return_type = match &return_type_capture {
            ReturnTypeCapture::Resolved(kt) => kt.clone(),
            ReturnTypeCapture::Unresolved(name) => match elaborate_type_expr(
                &mut elaborator,
                &crate::ast::TypeExpr::leaf(name.clone()),
            ) {
                ElabResult::Done(kt) => kt,
                ElabResult::Park(_) => {
                    return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                        "FN return type parked after Combine wake".to_string(),
                    )));
                }
                ElabResult::Unbound(_) => match KType::from_name(name) {
                    Some(kt) => kt,
                    None => {
                        return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                            "FN return-type slot: unknown type name `{name}`"
                        ))));
                    }
                },
            },
        };
        let elements = match signature::parse_fn_param_list(&spliced_signature, &mut elaborator) {
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

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "FN",
        ExpressionSignature {
            // FN returns a function value, but there's no "any function" KType anymore —
            // a function's structural type only exists once its signature is known. `Any`
            // here lets the constructed `KObject::KFunction`'s projected `ktype()` (which
            // does carry the full signature) flow through any caller's slot.
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("FN".into()),
                SignatureElement::Argument(Argument { name: "signature".into(),   ktype: KType::KExpression }),
                SignatureElement::Keyword("->".into()),
                SignatureElement::Argument(Argument { name: "return_type".into(), ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "body".into(),        ktype: KType::KExpression }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;

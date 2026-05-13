mod signature;

use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, Body, BodyResult, CombineFinish, KError, KErrorKind, KFunction, Scope, SchedulerHandle};
use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator};

use crate::runtime::machine::kfunction::argument_bundle::{extract_kexpression, extract_ktype};
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
    let return_type_raw = match extract_ktype(&mut bundle, "return_type") {
        Some(t) => t,
        None => {
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
        KType::Unresolved(name) => match elaborate_type_expr(
            &mut elaborator,
            &crate::ast::TypeExpr::leaf(name.clone()),
        ) {
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
        },
        kt => ReturnTypeState::Done(kt),
    };

    // Step 2: elaborate the parameter list.
    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => Ok(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Park(producers) => Err(producers),
    };

    // Step 3: route to synchronous-finalize or Combine-deferred based on parking state.
    match (return_type_state, params) {
        (ReturnTypeState::Done(rt), Ok(elements)) => {
            finalize_fn(scope, elements, rt, body_expr)
        }
        (ReturnTypeState::Done(rt), Err(producers)) => defer_via_combine(
            scope,
            sched,
            signature_expr,
            ReturnTypeCapture::Resolved(rt),
            producers,
            body_expr,
        ),
        (ReturnTypeState::Pending(name, mut producers), Ok(_)) => {
            // Param-types fully elaborated synchronously, but the return type parked. The
            // Combine finish re-runs both walks against the now-final scope for symmetry.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Unresolved(name),
                std::mem::take(&mut producers),
                body_expr,
            )
        }
        (ReturnTypeState::Pending(name, rt_producers), Err(mut producers)) => {
            producers.extend(rt_producers);
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                ReturnTypeCapture::Unresolved(name),
                producers,
                body_expr,
            )
        }
    }
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

/// Schedule a `Combine` over `producers` and re-run the signature elaboration in the
/// finish closure. Mirrors MODULE / SIG's `BodyResult::DeferTo` shape: the FN's terminal
/// lifts off the Combine's terminal, so the parent scope's binding lands at Combine-finish
/// time. The original `signature_expr` and `body_expr` are moved into the closure.
fn defer_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    signature_expr: crate::ast::KExpression<'a>,
    return_type_capture: ReturnTypeCapture,
    producers: Vec<crate::runtime::machine::NodeId>,
    body_expr: crate::ast::KExpression<'a>,
) -> BodyResult<'a> {
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        // Producers have finalized — re-elaborate against the now-stable scope. The
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
        let elements = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
            ParamListOutcome::Done(es) => es,
            ParamListOutcome::Err(msg) => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(msg)));
            }
            ParamListOutcome::Park(_) => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(
                    "FN signature elaboration parked again after Combine wake".to_string(),
                )));
            }
        };
        finalize_fn(scope, elements, return_type, body_expr.clone())
    });
    let combine_id = sched.add_combine(producers, scope, finish);
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

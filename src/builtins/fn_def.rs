mod finalize;
mod param_refs;
mod return_type;
mod signature;

use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::machine::model::types::Elaborator;

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin_with_pre_run, sig};

use finalize::{classify, defer_via_combine, finalize_fn, FnPlan, ParamListResult};
use return_type::{classify_return_type, extract_return_type_raw};
use signature::ParamListOutcome;

pub(crate) use signature::pre_run;

/// `FN <signature:KExpression> -> <return_type:Type> = <body:KExpression>` — the user-defined
/// function constructor. The signature and body slots are `KType::KExpression`, so the parser's
/// parenthesized sub-expressions match and ride through as data. Two return-type slot
/// overloads share this body:
///
///   1. `KType::TypeExprRef` — `Type(_)` carrier (`-> Number`, `-> List<Str>`, `-> Er`,
///      `-> SomeUserBound`). The carrier's structured `TypeExpr` is preserved through the
///      bundle for either eager elaboration (no parameter refs) or `ReturnType::Deferred(TypeExpr)`
///      (parameter refs detected — Stage B).
///   2. `KType::KExpression` — `Expression(_)` carrier (`-> (MODULE_TYPE_OF Er Type)`,
///      `-> (SIG_WITH Set ((Elt: Er)))`). Captured raw so the parens-form survives FN-def
///      without sub-dispatching against the outer scope where the parameter is unbound.
///      Routes either to `ReturnType::Deferred(Expression)` (parameter refs detected) or
///      to a `defer_via_combine` sub-Dispatch (no parameter refs, lifted to `Resolved`).
///
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
    let return_type_raw = match extract_return_type_raw(&mut bundle) {
        Ok(r) => r,
        Err(e) => return err(e),
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

    // Module-system functor-params Stage B: parameter-name scan. Extract parameter
    // names from the signature shape before any elaboration runs, then scan the
    // return-type carrier for any leaf matching a parameter name. A match short-
    // circuits the eager-elaborate path — the return type becomes
    // `ReturnType::Deferred(_)`, carried verbatim through to `KFunction::invoke`,
    // which re-elaborates against the per-call scope where Stage A's dual-write has
    // installed the parameter's type-language identity.
    //
    // The scan reads `signature_expr` raw rather than the elaborated `SignatureElement`
    // list so we can decide before triggering the elaborator's parking machinery — a
    // parked-on-placeholder param type still contributes its name to the scan.
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    // Phase-3 elaborator: parameter-type leaf names park on outstanding type-binding
    // placeholders, so a `LET MyList = (LIST_OF Number)` dispatched in the same batch
    // wakes the FN's signature elaboration when its body finalizes.
    let mut elaborator = Elaborator::new(scope);

    // Step 1: classify the return-type carrier. See [`ReturnTypeState`] for the
    // four outcomes and [`classify_return_type`] for the routing rules.
    let return_type_state = match classify_return_type(return_type_raw, &param_names, scope, sched) {
        Ok(s) => s,
        Err(e) => return err(e),
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

    // Step 3: plan the build — synchronous vs Combine-deferred — and execute.
    match classify(return_type_state, params) {
        FnPlan::Synchronous { elements, return_type } => {
            finalize_fn(scope, elements, return_type, body_expr)
        }
        FnPlan::Combine(inputs) => {
            defer_via_combine(scope, sched, signature_expr, inputs, body_expr)
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Overload 1: `Type(_)` or `TypeNameRef` return-type carrier (the historical surface).
    // FN-construction-time eager-elaborate path lifts the resolved `KType` into the
    // signature. Module-system functor-params Stage B: if the captured `TypeExpr` carries
    // a parameter-name leaf, the body switches to `ReturnType::Deferred(TypeExpr)` and
    // re-elaborates per call against the dispatch-boundary scope.
    // FN returns a function value, but there's no "any function" KType anymore —
    // a function's structural type only exists once its signature is known. `Any`
    // here lets the constructed `KObject::KFunction`'s projected `ktype()` (which
    // does carry the full signature) flow through any caller's slot.
    register_builtin_with_pre_run(
        scope,
        "FN",
        sig(KType::Any, vec![
            kw("FN"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
    // Overload 2: `Expression(_)` return-type carrier — parens-form return types
    // (`(MODULE_TYPE_OF Er Type)`, `(SIG_WITH Set ((Elt: Er)))`). The `KExpression` slot
    // accepts the parens-form raw, so FN-def never sub-dispatches the expression against
    // the outer scope (where the parameter name is unbound by construction). The same
    // `body` runs and branches on `bundle.get("return_type")`'s shape.
    //
    // Dispatch shape: a `Type(_)` part strictly matches overload 1's `TypeExprRef` slot;
    // an `Expression(_)` part strictly matches overload 2's `KExpression` slot. The
    // strict pass picks one or the other unambiguously; the wrap-path admission of an
    // `Expression(_)` into `TypeExprRef` (the pre-Stage-B fallback for parens-form return
    // types) becomes a tentative-fallback that loses to overload 2's strict win.
    //
    // `Future(KTypeValue(_))` post-Combine wakes (the picker re-walks against a spliced
    // signature) still admit only against overload 1's `TypeExprRef`, since
    // `KExpression` doesn't accept `Future(_)` of any kind. Pinned by
    // `fn_def/tests/module_stage2.rs` regression coverage.
    register_builtin_with_pre_run(
        scope,
        "FN",
        sig(KType::Any, vec![
            kw("FN"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::KExpression),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;

mod signature;

use crate::dispatch::{
    Argument, ArgumentBundle, Body, BodyResult, ExpressionSignature, KError, KErrorKind, KFunction,
    KObject, KType, Scope, SchedulerHandle, SignatureElement,
};
use crate::dispatch::types::ScopeResolver;

use crate::dispatch::kfunction::argument_bundle::{extract_kexpression, extract_type_expr};
use super::{err, register_builtin_with_pre_run};

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
    _sched: &mut dyn SchedulerHandle<'a>,
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
    let return_type_expr = match extract_type_expr(&mut bundle, "return_type") {
        Some(t) => t,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FN return-type slot must be a type expression (e.g. `Number`, `List<Str>`)"
                    .to_string(),
            )));
        }
    };
    // ScopeResolver walks the surrounding scope's bindings first so user-defined types
    // (`LET MyList = (LIST_OF Number)`) shadow builtins. Stage-2 substrate per the
    // [module-system stage 2 plan](../../../../roadmap/module-system-2-scheduler.md).
    let resolver = ScopeResolver::new(scope);
    let return_type = match KType::from_type_expr(&return_type_expr, &resolver) {
        Ok(t) => t,
        Err(msg) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "FN return-type slot: {msg}"
            ))));
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

    let elements = match signature::parse_fn_param_list(&signature_expr, &resolver) {
        Ok(es) => es,
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };
    // Pick the first Keyword as the data-table key. `scope.functions` does the load-bearing
    // dispatch lookup by signature; `scope.data` is mostly for discoverability and
    // shadow-by-name semantics, neither of which has a single right answer for a multi-token
    // signature like `(a ADD b)`. First Keyword is a defensible default.
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

//! `FUNCTOR <signature:KExpression> -> <return_type:Type> = <body:KExpression>` —
//! the user-defined functor constructor. Surface and dispatch shape parallel
//! [`crate::builtins::fn_def`]; the divergences from FN are:
//!
//! 1. The constructed `KFunction` carries `is_functor: true`, so its
//!    `function_value_ktype` projects to `KType::KFunctor`.
//! 2. The return-type slot is validated at the FUNCTOR site against the
//!    admissible-carrier list from
//!    [design/typing/functors.md](../../design/typing/functors.md). Other carriers
//!    error here, before the body has a chance to surface a frames-removed
//!    `TypeMismatch`.
//!
//! Validation is fused into [`classify_return_type`]: passing
//! `Some(&param_type_map)` emits a `Rejected`/`Admissible`/`DeferredToCombine`
//! verdict alongside classification so the carrier is walked once. The deferred
//! arm rides Combine-finish gated by the same `FnKind::Functor` — no separate
//! predicate closure threaded through `defer_via_combine`.

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::Elaborator;
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::fn_def::finalize::{
    classify, defer_via_combine, finalize_fn_with_kind, FnKind, FnPlan, ParamListResult,
};
use super::fn_def::return_type::{
    classify_return_type, extract_return_type_raw, AdmissibleVerdict,
};
use super::fn_def::signature::{
    collect_param_names_from_signature, parse_fn_param_list, ParamListOutcome,
};
use super::{arg, err, kw, register_builtin_full, sig};

/// FUNCTOR binder body. Mirrors [`crate::builtins::fn_def::body`] except for
/// the return-type validation and the `is_functor: true` flag.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let signature_expr = match extract_kexpression(&mut bundle, "signature") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "FUNCTOR signature slot must be a parenthesized expression".to_string(),
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
                "FUNCTOR body slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let param_names = collect_param_names_from_signature(&signature_expr);

    // Param-name → declared-`KType` map for the deferred-arm head inspector's
    // "is this bare-param ref type-denoting?" check on slots like `-> Er`.
    // Only includes slots whose type elaborates eagerly through `Elaborator`.
    let param_type_map = collect_param_types(&signature_expr, scope);

    // Gate param type names to the FUNCTOR's lexical position.
    let mut elaborator = Elaborator::new(scope).with_chain(sched.current_lexical_chain());

    // `Some(&map)` activates the FUNCTOR-return verdict. `Rejected` short-circuits;
    // `DeferredToCombine` rides Combine-finish via the `is_functor` flag below.
    let (return_type_state, verdict) =
        match classify_return_type(return_type_raw, &param_names, scope, Some(&param_type_map)) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
    if let AdmissibleVerdict::Rejected(e) = verdict {
        return err(e);
    }

    let params = match parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => ParamListResult::Pending {
            park_producers,
            sub_dispatches,
        },
    };

    // Non-nominal: the FUNCTOR name obeys source order like any other type name.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    match classify(return_type_state, params) {
        FnPlan::Synchronous {
            elements,
            return_type,
        } => finalize_fn_with_kind(
            scope,
            elements,
            return_type,
            body_expr,
            FnKind::Functor,
            bind_index,
        ),
        FnPlan::Combine(inputs) => {
            // `FnKind::Functor` gates the resolved-return admissibility check
            // at Combine-finish — no separate predicate closure threaded here.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                inputs,
                body_expr,
                FnKind::Functor,
                bind_index,
            )
        }
    }
}

/// Build a map of `param_name → declared-KType` for the deferred-arm head
/// inspector. Skips slots that don't elaborate eagerly; the Combine path's
/// resolved validator catches the slack.
fn collect_param_types<'a>(
    signature: &KExpression<'a>,
    scope: &'a Scope<'a>,
) -> std::collections::HashMap<String, KType<'a>> {
    use crate::machine::model::types::{elaborate_type_expr, ElabResult};
    let mut map = std::collections::HashMap::new();
    let mut el = Elaborator::new(scope);
    let parts = &signature.parts;
    let mut i = 0;
    while i < parts.len() {
        let param_name: Option<String> = match &parts[i].value {
            ExpressionPart::Identifier(name) => Some(name.clone()),
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        };
        if let Some(name) = param_name {
            if let Some(next_part) = parts.get(i + 1) {
                if let ExpressionPart::Type(t) = &next_part.value {
                    if let ElabResult::Done(kt) = elaborate_type_expr(&mut el, t) {
                        map.insert(name, kt);
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    map
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Two overloads mirror FN: `TypeExprRef` for `-> Number` / `-> Er` /
    // `-> :(Functor ...)` and `KExpression` for parens-form carriers like
    // `-> (SIG_WITH …)`. `binder_bucket` lets a sibling bare-arg call park on
    // a still-finalizing overload; sibling overloads sharing a bucket key all
    // install for it and only the first finalize wins. No `binder_name` —
    // FUNCTOR registers under `functions[bucket]`, not a value-side carrier.
    register_builtin_full(
        scope,
        "FUNCTOR",
        sig(
            KType::Any,
            vec![
                kw("FUNCTOR"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::TypeExprRef),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        None,
        Some(super::fn_def::binder_bucket),
        false,
    );
    register_builtin_full(
        scope,
        "FUNCTOR",
        sig(
            KType::Any,
            vec![
                kw("FUNCTOR"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::KExpression),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        None,
        Some(super::fn_def::binder_bucket),
        false,
    );
}

#[cfg(test)]
mod tests;

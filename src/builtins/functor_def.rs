//! `FUNCTOR <signature:KExpression> -> <return_type:Type> = <body:KExpression>` —
//! the user-defined functor constructor. Surface and dispatch shape parallel
//! [`crate::builtins::fn_def`] verbatim: signature parsing rides
//! [`super::fn_def::signature::parse_fn_param_list`], return-type classification
//! rides [`super::fn_def::return_type::classify_return_type`], and the deferred /
//! synchronous split rides [`super::fn_def::finalize::classify`] +
//! [`super::fn_def::finalize::finalize_fn_with_flag`]. The only differences
//! from FN are:
//!
//! 1. The constructed `KFunction` carries `is_functor: true` (set by passing
//!    `true` through `finalize_fn_with_flag` / `defer_via_combine`), so its
//!    `function_value_ktype` projects to `KType::KFunctor`.
//! 2. The return-type slot is validated at the FUNCTOR site against the
//!    admissible-carrier list from
//!    [design/typing/functors.md](../../design/typing/functors.md): module
//!    (`AnyModule`, `Module`), signature (`AnySignature`, `SatisfiesSignature`,
//!    `Signature`), and recursively functor (`KFunctor`). Other carriers
//!    error here, before the body has a chance to surface a frames-removed
//!    `TypeMismatch`.
//!
//! Validation runs in three arms (no `Box<dyn Fn>` plumbing — see the parallel
//! header comment in [`super::fn_def::finalize`]):
//!
//! - **Resolved** (`ReturnTypeState::Done`): walk the `KType` against
//!   [`KType::is_admissible_functor_return`] synchronously, here.
//! - **Deferred** (`ReturnTypeState::Deferred`): inspect the surface-form head
//!   of the captured `DeferredReturn`. `SIG_WITH` admissible; `MODULE_TYPE_OF`
//!   not (it produces an `AbstractType`); `(Functor …)` sigil admissible; a
//!   bare-param ref admissible iff the param's declared type is type-denoting
//!   (`KType::is_type_denoting`). Best-effort: a bare leaf whose declared
//!   param type itself parks falls through to the per-call dispatch boundary's
//!   `matches_value` safety net rather than rejecting at the FUNCTOR site.
//! - **Pending / ExprToSubDispatch** (Combine path): validation runs at the
//!   Combine-finish boundary inside `finalize_fn_with_flag`, gated by the
//!   `is_functor: true` flag we thread through `defer_via_combine`. Same
//!   admissibility predicate, called on the resolved `KType`. No closure
//!   allocation per FUNCTOR — the flag the FUNCTOR builtin already passes is
//!   the only signal `finalize_fn_with_flag` needs.

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::model::types::{DeferredReturn, Elaborator};
use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use super::fn_def::finalize::{
    classify, defer_via_combine, finalize_fn_with_flag, FnPlan, ParamListResult,
};
use super::fn_def::return_type::{classify_return_type, extract_return_type_raw, ReturnTypeState};
use super::fn_def::signature::{
    collect_param_names_from_signature, parse_fn_param_list, ParamListOutcome,
};
use super::{arg, err, kw, register_builtin_with_pre_run, sig};

/// The body of a FUNCTOR binder. Mirrors [`crate::builtins::fn_def::body`]
/// step-for-step; the only divergences are the keyword strings in
/// diagnostics, the post-classification return-type validation, and the
/// `is_functor: true` flag threaded through finalize.
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
    let body_expr = super::cons::fold_multi_statement(body_expr);

    let param_names = collect_param_names_from_signature(&signature_expr);

    // Build a param-name → declared-`KType` map up front, so the deferred-arm
    // head inspector can answer "is this bare-param ref type-denoting?" for
    // FUNCTOR return slots like `-> Er`. Bare-leaf type names like `Er` show
    // up as `Type(TypeExpr { name, params: TypeParams::None })`; the position
    // immediately follows a parameter-name slot, mirroring the structural
    // walk in `collect_param_names_from_signature`. We can't yet elaborate
    // unresolved type-class names against the captured scope (they may not be
    // bound), so the map only includes slots whose type elaborates eagerly
    // through `Elaborator`. That covers `:OrderedSig`, `:Module`, etc — the
    // surface forms relevant for the type-denoting check.
    let param_type_map = collect_param_types(&signature_expr, scope);

    let mut elaborator = Elaborator::new(scope);

    let return_type_state = match classify_return_type(return_type_raw, &param_names, scope) {
        Ok(s) => s,
        Err(e) => return err(e),
    };

    // Synchronous-arm validation: a `Done` carrier validates against the
    // admissible-carrier list now; a `Deferred` carrier validates its
    // surface-form head now. The two remaining states (`Pending`,
    // `ExprToSubDispatch`) ride the Combine path; the `is_functor: true` flag
    // threaded through `defer_via_combine` makes `finalize_fn_with_flag`
    // re-run the admissibility check on the resolved `KType` at Combine-finish
    // time — no validator closure needed.
    match &return_type_state {
        ReturnTypeState::Done(kt) => {
            if !kt.is_admissible_functor_return() {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTOR return-type slot must denote a module, signature, or functor; got `{}`",
                    kt.name(),
                ))));
            }
        }
        ReturnTypeState::Deferred(d) => {
            if let Err(e) = validate_deferred_return_head(d, &param_type_map) {
                return err(e);
            }
        }
        ReturnTypeState::Pending { .. } | ReturnTypeState::ExprToSubDispatch(_) => {
            // Validated at Combine finish inside `finalize_fn_with_flag`'s
            // `is_functor` arm — see the post-Combine path on
            // `defer_via_combine` below.
        }
    }

    let params = match parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

    match classify(return_type_state, params) {
        FnPlan::Synchronous { elements, return_type } => {
            finalize_fn_with_flag(scope, elements, return_type, body_expr, true)
        }
        FnPlan::Combine(inputs) => {
            // `is_functor: true` triggers the FUNCTOR-specific resolved-return
            // admissibility check inside `finalize_fn_with_flag` when the
            // Combine finish lands. No `Box<dyn Fn>` closure — the predicate
            // is a method call on `KType` keyed on the flag.
            defer_via_combine(scope, sched, signature_expr, inputs, body_expr, true)
        }
    }
}

/// Deferred-arm validator. Pattern-match the surface form of the captured
/// `DeferredReturn` against the admissible heads.
///
/// `TypeExpr` arm — a bare-leaf `Er` matching a parameter name admits iff
/// that parameter's declared `KType` is type-denoting (e.g. `:OrderedSig`,
/// `:Module`). A `Functor`-headed parameterized form admits via the
/// type-position sigil. Other shapes are rejected here so the diagnostic
/// surfaces at the FUNCTOR site.
///
/// `Expression` arm — inspect the leading `Keyword`. `SIG_WITH` produces a
/// `SatisfiesSignature` so admits; `MODULE_TYPE_OF` produces an
/// `AbstractType` and is rejected.
fn validate_deferred_return_head<'a>(
    d: &DeferredReturn<'a>,
    param_type_map: &std::collections::HashMap<String, KType<'a>>,
) -> Result<(), KError> {
    match d {
        DeferredReturn::TypeExpr(te) => validate_deferred_type_expr(te, param_type_map),
        DeferredReturn::Expression(e) => validate_deferred_expression(e),
    }
}

/// Inspect a `TypeExpr` for the FUNCTOR-return admissibility rules. Same shape
/// as `is_admissible_functor_return` but operating on the surface form rather
/// than an elaborated `KType` — so the rules look at parameter shapes
/// (`TypeParams::None` → look up the param's declared type; `TypeParams::Function`
/// with head `Functor` → admissible; everything else → rejected).
fn validate_deferred_type_expr<'a>(
    te: &TypeExpr,
    param_type_map: &std::collections::HashMap<String, KType<'a>>,
) -> Result<(), KError> {
    match &te.params {
        TypeParams::None => {
            // Bare-leaf reference. If it matches a parameter name, admit iff
            // the parameter's declared type is type-denoting (signature- or
            // module-typed). If it doesn't match a parameter, defer the check
            // to Combine-finish via the resolved validator — the head can't
            // be authoritatively classified pre-elaboration.
            if let Some(param_kt) = param_type_map.get(&te.name) {
                if param_kt.is_type_denoting() {
                    return Ok(());
                }
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "FUNCTOR return-type slot must denote a module, signature, or functor; \
                     parameter `{}` is declared as `{}`, which is not type-denoting",
                    te.name,
                    param_kt.name(),
                ))));
            }
            // Non-parameter bare-leaf — falls through to the synchronous
            // `Done` arm in practice (`classify_return_type` would have
            // resolved it eagerly); reaching this branch from the Deferred
            // arm means the param-name scan matched but the map didn't,
            // which only happens if elaboration of the param's type slot
            // failed eagerly. Admit conservatively; downstream resolution
            // will surface a structured error if the carrier is invalid.
            Ok(())
        }
        TypeParams::Function { .. } if te.name == "Functor" => Ok(()),
        TypeParams::Function { .. } | TypeParams::List(_) => {
            Err(KError::new(KErrorKind::ShapeError(format!(
                "FUNCTOR return-type slot must denote a module, signature, or functor; got `{}`",
                te.render(),
            ))))
        }
    }
}

/// Inspect a parens-form return-type carrier (`(SIG_WITH …)`,
/// `(MODULE_TYPE_OF …)`, etc) for the FUNCTOR-return admissibility rules.
/// Head-keyword classification: `SIG_WITH` → admissible (yields
/// `SatisfiesSignature`); `MODULE_TYPE_OF` → rejected (yields `AbstractType`).
/// Other heads fall through to a generic rejection — the parens-form return
/// carriers actually used as functor returns in shipped code are the two
/// above and the bare-parameter / sigil shapes covered in the `TypeExpr` arm.
fn validate_deferred_expression(e: &KExpression<'_>) -> Result<(), KError> {
    let head_keyword = e.parts.iter().find_map(|p| match &p.value {
        ExpressionPart::Keyword(s) => Some(s.as_str()),
        _ => None,
    });
    match head_keyword {
        Some("SIG_WITH") => Ok(()),
        Some("MODULE_TYPE_OF") => Err(KError::new(KErrorKind::ShapeError(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             `MODULE_TYPE_OF` produces an abstract type, not a module or signature"
                .to_string(),
        ))),
        Some(other) => Err(KError::new(KErrorKind::ShapeError(format!(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             head keyword `{other}` does not produce a module, signature, or functor",
        )))),
        None => Err(KError::new(KErrorKind::ShapeError(
            "FUNCTOR return-type slot must denote a module, signature, or functor; \
             return-type expression has no recognizable head"
                .to_string(),
        ))),
    }
}

/// Walk the signature's part list once to build a map of `param_name →
/// declared-KType`. Only includes parameters whose type slot is a bare
/// `TypeExpr` and resolves eagerly through `Elaborator`; parens-wrapped /
/// forward-referencing type slots are skipped (they ride the Combine path,
/// where the resolved validator picks up the slack).
///
/// Mirrors the walk in
/// [`super::fn_def::signature::collect_param_names_from_signature`] but also
/// captures the elaborated `KType` so the deferred-arm head inspector can
/// answer the type-denoting question.
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
            ExpressionPart::Type(t) if matches!(t.params, TypeParams::None) => {
                Some(t.name.clone())
            }
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
    // Two overloads mirror FN's: `TypeExprRef` for `-> Number` / `-> Er` /
    // `-> :(Functor ...)` shapes and `KExpression` for parens-form carriers
    // like `-> (SIG_WITH …)`. Same dispatch tie-break rules apply — the
    // strict dispatch pass picks one; `Future(KTypeValue(_))` post-Combine
    // wakes admit only against `TypeExprRef`.
    //
    // Pre-run hook is the same `fn_def::pre_run` extractor — both binders
    // place the signature at `parts[1]` and the first `Keyword` in that
    // signature names the registered function.
    register_builtin_with_pre_run(
        scope,
        "FUNCTOR",
        sig(KType::Any, vec![
            kw("FUNCTOR"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::TypeExprRef),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(super::fn_def::pre_run),
    );
    register_builtin_with_pre_run(
        scope,
        "FUNCTOR",
        sig(KType::Any, vec![
            kw("FUNCTOR"),
            arg("signature", KType::KExpression),
            kw("->"),
            arg("return_type", KType::KExpression),
            kw("="),
            arg("body", KType::KExpression),
        ]),
        body,
        Some(super::fn_def::pre_run),
    );
}

#[cfg(test)]
mod tests;

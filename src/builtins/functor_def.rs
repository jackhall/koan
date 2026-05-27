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
//! Validation is fused into [`classify_return_type`]: passing
//! `Some(&param_type_map)` triggers the FUNCTOR-return verdict emission
//! alongside classification, so the carrier is walked once. The verdict has
//! three shapes:
//!
//! - **`Admissible`** — synchronously admissible (`Done` arm passes
//!   [`KType::is_admissible_functor_return`]; `Deferred` arm's surface form
//!   passes the head inspector).
//! - **`Rejected`** — synchronously rejected with the formatted diagnostic.
//! - **`DeferredToCombine`** — final check rides Combine-finish inside
//!   `finalize_fn_with_flag`, gated by the `is_functor: true` flag threaded
//!   through `defer_via_combine`. No `Box<dyn Fn>` plumbing — the flag the
//!   FUNCTOR builtin already passes is the only signal needed.

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeParams};
use crate::machine::model::types::Elaborator;
use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, Scope, SchedulerHandle,
};

use super::fn_def::finalize::{
    classify, defer_via_combine, finalize_fn_with_flag, FnPlan, ParamListResult,
};
use super::fn_def::return_type::{
    classify_return_type, extract_return_type_raw, AdmissibleVerdict,
};
use super::fn_def::signature::{
    collect_param_names_from_signature, parse_fn_param_list, ParamListOutcome,
};
use super::{arg, err, kw, register_builtin_full, sig};

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
    // Multi-statement FUNCTOR bodies (`((s_0) ... (s_{N-1}))`) split at
    // `KFunction::invoke` time, same as FN bodies — no CONS-fold here.

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

    // Fused classify + FUNCTOR-return verdict emission. `Some(&map)` activates
    // the verdict; `Rejected` short-circuits here. `Admissible` and
    // `DeferredToCombine` both proceed — the latter rides Combine-finish via
    // the `is_functor: true` flag threaded through `defer_via_combine`.
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
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

    // FUNCTOR's bind_index: lexical position of the executing slot with the D7
    // nominal-binder carve-out so siblings on the same block see one another
    // regardless of source order (mutual recursion across FUNCTORs).
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::nominal(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    match classify(return_type_state, params) {
        FnPlan::Synchronous { elements, return_type } => {
            finalize_fn_with_flag(scope, elements, return_type, body_expr, true, bind_index)
        }
        FnPlan::Combine(inputs) => {
            // `is_functor: true` triggers the FUNCTOR-specific resolved-return
            // admissibility check inside `finalize_fn_with_flag` when the
            // Combine finish lands. No `Box<dyn Fn>` closure — the predicate
            // is a method call on `KType` keyed on the flag.
            defer_via_combine(
                scope,
                sched,
                signature_expr,
                inputs,
                body_expr,
                true,
                bind_index,
            )
        }
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
    // FUNCTOR mirrors FN: register a function by inner-call bucket key
    // (UntypedKey). Both binders supply `binder_bucket` so a sibling bare-arg
    // call to a still-finalizing FUNCTOR overload parks on this slot's bucket
    // entry — `(MAKESET IntOrd)` to a `FUNCTOR (MAKESET Er :OrderedSig) ->
    // (SIG_WITH …)` binder whose body is parked on a SIG-body Combine. Multiple
    // sibling FUNCTOR overloads sharing one bucket key all install for it; the
    // first to finalize writes `functions[bucket]` and the others' installs
    // become idempotent no-ops.
    //
    // No `binder_name` install — same rationale as FN (see fn_def::register).
    // FUNCTOR does not bind a single name to a value-side carrier; it registers
    // a callable function in `functions[bucket]`. A name placeholder would
    // Rebind across sibling overloads.
    register_builtin_full(
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
        None,
        Some(super::fn_def::binder_bucket),
        false,
        // FUNCTOR is a nominal binder (D7 carve-out): siblings can refer to one
        // another regardless of source order — `FUNCTOR A` body can mention `B`
        // declared after it on the same block. The carve-out rides on
        // `BindingIndex.nominal_binder`, not on the (absent) `binder_name`.
        true,
    );
    register_builtin_full(
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
        None,
        Some(super::fn_def::binder_bucket),
        false,
        // See above.
        true,
    );
}

#[cfg(test)]
mod tests;

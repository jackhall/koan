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
/// function constructor. Signature and body are captured as raw `KExpression`s; the signature
/// is structurally inspected (never dispatched) to derive the registered function's
/// `ExpressionSignature`, and `KFunction::invoke` re-dispatches the body per call with
/// parameters bound into a per-call child scope.
///
/// At least one `Keyword` is required in the signature: a signature of all-Argument slots
/// would shadow `value_lookup`/`value_pass`, so the dispatcher needs a fixed token to key
/// on. Bare identifiers without `: Type`, stray type tokens, literals, and nested
/// expressions in the signature are rejected with a `ShapeError`.
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
    // A body of bare sub-expressions (`((a) (b) (c))`) right-folds into a `CONS` chain
    // so the scheduler sees a single tail-callable expression: TCO holds on the last
    // statement; backward refs across statements work, forward refs do not.
    let body_expr = super::cons::fold_multi_statement(body_expr);

    // Parameter-name scan runs against the raw signature before elaboration so a
    // param type that's still parked on a placeholder still contributes its name.
    // A match in the return-type carrier defers elaboration to `KFunction::invoke`,
    // where the per-call scope has the parameter's type-language identity bound.
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    let mut elaborator = Elaborator::new(scope);

    let return_type_state = match classify_return_type(return_type_raw, &param_names, scope, sched) {
        Ok(s) => s,
        Err(e) => return err(e),
    };

    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

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
    // FN's declared return is `KType::Any`: a function's structural type only exists
    // once its signature is known, so there's no "any function" KType to use here.
    // The constructed `KObject::KFunction` projects its full signature through
    // `ktype()` at the call site.
    //
    // Two overloads cover the return-type carrier: `TypeExprRef` for `Type(_)`
    // (`-> Number`, `-> Er`, ...) and `KExpression` for parens-form
    // (`-> (MODULE_TYPE_OF Er Type)`). The strict dispatch pass picks one
    // unambiguously; `Future(KTypeValue(_))` post-Combine wakes admit only against
    // `TypeExprRef`, since `KExpression` doesn't accept `Future(_)`.
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

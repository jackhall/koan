pub(crate) mod finalize;
mod param_refs;
pub(crate) mod return_type;
pub(crate) mod signature;

use crate::machine::model::KType;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, Scope, SchedulerHandle,
};
use crate::machine::model::types::Elaborator;

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin_full, sig};

use finalize::{classify, defer_via_combine, finalize_fn, FnPlan, ParamListResult};
use return_type::{classify_return_type, extract_return_type_raw};
use signature::ParamListOutcome;

pub(crate) use signature::binder_bucket;
#[cfg(test)]
pub(crate) use signature::binder_name;

/// At least one `Keyword` is required in the signature: an all-Argument signature
/// would shadow `value_pass` (or the `BareIdentifier`/`BareTypeLeaf` fast lanes for
/// the single-arg case), so the dispatcher needs a fixed token to key on.
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
    // Scan param names against the raw signature: a param type still parked on a
    // placeholder still contributes its name, which the return-type carrier may
    // reference (deferring elaboration to per-call scope at invoke time).
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    let mut elaborator = Elaborator::new(scope);

    // `None` verdict context: FUNCTOR's arm consumes a verdict computed against
    // `Some(&param_type_map)`; FN computes a no-op `Admissible` and drops it.
    let (return_type_state, _verdict) =
        match classify_return_type(return_type_raw, &param_names, scope, None) {
            Ok(p) => p,
            Err(e) => return err(e),
        };

    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
        ParamListOutcome::Pending { park_producers, sub_dispatches } => {
            ParamListResult::Pending { park_producers, sub_dispatches }
        }
    };

    // Value-style bind_index: FN produces a callable but registers no
    // sibling-visible nominal identity, so no D7 carve-out applies.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    match classify(return_type_state, params) {
        FnPlan::Synchronous { elements, return_type } => {
            finalize_fn(scope, elements, return_type, body_expr, bind_index)
        }
        FnPlan::Combine(inputs) => {
            defer_via_combine(scope, sched, signature_expr, inputs, body_expr, false, bind_index)
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Declared return is `KType::Any`: a function's structural type only exists
    // once its signature is known. The constructed `KObject::KFunction` projects
    // its full signature through `ktype()` at the call site.
    //
    // Two overloads cover the return-type carrier — `TypeExprRef` for `Type(_)`
    // (`-> Number`), `KExpression` for parens-form (`-> (MODULE_TYPE_OF Er Type)`).
    // `Future(KTypeValue(_))` post-Combine wakes admit only against `TypeExprRef`.
    //
    // FN supplies only `binder_bucket` (no `binder_name`): sibling FN overloads
    // sharing one bucket each install their own per-bucket entry, and consumers
    // pick the earliest-index visible entry to park on. A `binder_name` install
    // would Rebind on the second sibling sharing a head keyword (two `PICK`
    // overloads both claiming `placeholders[PICK]`), collapsing the overload set.
    // LET / STRUCT / UNION / SIG / MODULE keep `binder_name` because they bind
    // exactly one name to a value-side carrier; sibling collisions there are
    // real Rebind errors, not overload patterns.
    //
    // The final `false` is the nominal-binder flag: FN is value-side gated, so
    // `LET f = (FN ...)` does not register a sibling-visible nominal identity.
    register_builtin_full(
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
        None,
        Some(binder_bucket),
        false,
        false,
    );
    register_builtin_full(
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
        None,
        Some(binder_bucket),
        false,
        false,
    );
}

#[cfg(test)]
mod tests;

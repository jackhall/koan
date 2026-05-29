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
// `binder_name` is kept in the module (signature.rs) for the parser-side smoke
// test; it is intentionally NOT registered as a hook on FN/FUNCTOR overloads —
// see the `register` function below for the rationale.
#[cfg(test)]
pub(crate) use signature::binder_name;

/// `FN <signature:KExpression> -> <return_type:Type> = <body:KExpression>` — the user-defined
/// function constructor. Signature and body are captured as raw `KExpression`s; the signature
/// is structurally inspected (never dispatched) to derive the registered function's
/// `ExpressionSignature`, and `KFunction::invoke` re-dispatches the body per call with
/// parameters bound into a per-call child scope.
///
/// At least one `Keyword` is required in the signature: a signature of all-Argument slots
/// would shadow `value_pass` (or the `BareIdentifier`/`BareTypeLeaf` fast lanes for the
/// single-arg case), so the dispatcher needs a fixed token to key on. Bare identifiers
/// without `: Type`, stray type tokens, literals, and nested expressions in the signature
/// are rejected with a `ShapeError`.
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
    // Multi-statement bodies (`((a) (b) (c))`) are split at FN-invoke time by
    // `KFunction::invoke`: the first N-1 statements are submitted as siblings into
    // the per-call body scope via `enter_block`, and the FN slot tail-replaces into
    // the last statement, preserving TCO. No CONS-fold at construction time.

    // Parameter-name scan runs against the raw signature before elaboration so a
    // param type that's still parked on a placeholder still contributes its name.
    // A match in the return-type carrier defers elaboration to `KFunction::invoke`,
    // where the per-call scope has the parameter's type-language identity bound.
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    let mut elaborator = Elaborator::new(scope);

    // FN passes `None` for the FUNCTOR-return verdict context — the verdict
    // is computed as a no-op `Admissible` and dropped. The FUNCTOR builtin
    // (see `functor_def::body`) passes `Some(&param_type_map)` and consumes
    // the verdict in the same arm.
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

    // FN's bind_index: lexical position of the executing slot. FN bindings are
    // value-style gated (no D7 carve-out) — even though FN produces a callable,
    // the binder itself never registers a sibling-visible nominal identity.
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
    // FN registers a function by inner-call bucket key (UntypedKey), NOT by name:
    // sibling FN overloads (`FN (PICK xs :A) -> ...`, `FN (PICK xs :B) -> ...`)
    // coexist in `functions[bucket]`. `register_function` mirrors the first such
    // overload's carrier into `data[name]` as a value-side handle for `LET f = (FN
    // ...)`-style references, but that mirror is incidental — the dispatch-time
    // identity is the bucket, not the name.
    //
    // Consequently FN supplies only `binder_bucket` (no `binder_name`):
    // - `binder_bucket` installs a `pending_overloads[key]` entry so a sibling
    //   bare-arg call parks on this binder's slot by inner-call bucket key while
    //   the body is still resolving (e.g. parked on a parameter-type sub-Dispatch).
    //   Multiple sibling FNs sharing one bucket each install their OWN entry
    //   into the per-bucket Vec; consumers pick the earliest-index visible
    //   entry to park on. On a sibling's finalize, only its own entry is
    //   removed from the Vec; other siblings stay pending so the next consumer
    //   continues to find them as wake sources. This is the "index-gated
    //   bucket parking" pattern — sibling installs are distinct, not
    //   coalesced.
    // - A `binder_name` install would Rebind on the second of two sibling FN
    //   binders sharing the same head keyword (e.g. two `PICK` overloads): both
    //   would try to claim `placeholders[PICK]` and the second's install would
    //   error out the slot, leaving only the first overload registered. Since
    //   FN binds nothing in `data[name]` that needs a forward-reference wake
    //   beyond what `binder_bucket` already provides, no name placeholder is
    //   installed.
    //
    // LET / STRUCT / UNION / SIG / MODULE keep `binder_name` because they DO
    // bind exactly one name to a value-side carrier; sibling collisions there
    // are real Rebind errors, not overload patterns.
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
        // FN is *not* a nominal binder: a `LET f = (FN ...)` form is value-side gated.
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
        // FN is *not* a nominal binder: a `LET f = (FN ...)` form is value-side gated.
        false,
    );
}

#[cfg(test)]
mod tests;

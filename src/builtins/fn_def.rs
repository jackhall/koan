pub(crate) mod finalize;
mod param_refs;
pub(crate) mod return_type;
pub(crate) mod signature;

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::Elaborator;
use crate::machine::model::types::KKind;
use crate::machine::model::{Argument, KType, SignatureElement};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_full, sig};
use crate::machine::core::kfunction::argument_bundle::extract_ktype;

use finalize::{
    classify, defer_via_combine, finalize_fn_with_kind, FnKind, FnPlan, ParamListResult,
};
use return_type::{classify_return_type, extract_return_type_raw, AdmissibleVerdict};
use signature::ParamListOutcome;

pub(crate) use signature::binder_bucket;
#[cfg(test)]
pub(crate) use signature::binder_name;

/// Keyworded FN body: the parenthesized `(<signature>)` form, which registers
/// under its lead keyword. At least one `Keyword` is required — an all-Argument
/// signature has no fast-lane shape to key on (every keyword-free expression
/// routes through `BareIdentifier` / `BareTypeLeaf` / `LiteralPassThrough` /
/// `TypeCall` / `FunctionValueCall` / `SigiledTypeExpr`), so the dispatcher needs
/// a fixed token. The keyword-less `FN :{…}` record-schema form is
/// [`body_record_schema`].
pub fn body<'a, 's>(
    scope: &'s Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    build_fn_like(scope, sched, bundle, "FN", FnKind::Function)
}

/// Shared FN / FUNCTOR elaboration: extract the `signature` / return / `body`
/// slots, collect param names, classify the return type, parse the param list,
/// and route to [`finalize_fn_with_kind`] (synchronous) or [`defer_via_combine`]
/// (Combine). `kind` is the sole behavioral fork — `FnKind::Functor` builds the
/// param-type map and acts on the return-admissibility verdict; FN passes `None`
/// and [`classify_return_type`] returns `Admissible`, so the `Rejected` check is a
/// no-op. `builtin` (`"FN"` / `"FUNCTOR"`) names the surface in slot errors.
pub(crate) fn build_fn_like<'a, 's>(
    scope: &'s Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
    builtin: &str,
    kind: FnKind,
) -> BodyResult<'a> {
    let signature_expr = match bundle.extract_kexpression_or_shape_error(builtin, "signature") {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let return_type_raw = match extract_return_type_raw(&mut bundle) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let body_expr = match bundle.extract_kexpression_or_shape_error(builtin, "body") {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    // Scan param names against the raw signature: a param type still parked on a
    // placeholder still contributes its name, which the return-type carrier may
    // reference (deferring elaboration to per-call scope at invoke time).
    let param_names = signature::collect_param_names_from_signature(&signature_expr);

    // FUNCTOR-only: param-name → declared-`KType` map drives the deferred-arm head
    // inspector's "is this bare-param ref type-denoting?" check (slots like `-> Er`).
    // `Some(&map)` activates the FUNCTOR-return verdict; FN passes `None`.
    let param_type_map = match kind {
        FnKind::Functor => Some(collect_param_types(&signature_expr, scope)),
        FnKind::Function | FnKind::Anonymous => None,
    };

    // Gate param type names to the binder's lexical position — a parameter naming a
    // later type is a position error, like any other forward type reference.
    let mut elaborator = Elaborator::new(scope).with_chain(sched.current_lexical_chain());

    let (return_type_state, verdict) = match classify_return_type(
        return_type_raw,
        &param_names,
        scope,
        sched.current_lexical_chain(),
        param_type_map.as_ref(),
    ) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    // `Rejected` short-circuits the FUNCTOR; `DeferredToCombine` rides Combine-finish
    // via the `FnKind::Functor` gate in `finalize_fn_with_kind` / `defer_via_combine`.
    if let AdmissibleVerdict::Rejected(e) = verdict {
        return err(e);
    }

    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator) {
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

    // The binder name binds at its own lexical position, like every other binder.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    match classify(return_type_state, params) {
        FnPlan::Synchronous {
            elements,
            return_type,
        } => finalize_fn_with_kind(scope, elements, return_type, body_expr, kind, bind_index),
        FnPlan::Combine(inputs) => defer_via_combine(
            scope,
            sched,
            signature_expr,
            inputs,
            body_expr,
            kind,
            bind_index,
        ),
    }
}

/// Build a map of `param_name → declared-KType` for the FUNCTOR deferred-arm head
/// inspector. Skips slots that don't elaborate eagerly; the Combine path's
/// resolved validator catches the slack.
fn collect_param_types<'a>(
    signature: &KExpression<'a>,
    scope: &Scope<'a>,
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

/// Anonymous-FN body: `FN :{<record schema>} -> ReturnType = (<body>)`.
///
/// The record-schema sigil `:{…}` resolves to a `KType::Record` before this
/// fires — it is a first-class `ExpressionPart::RecordType` the dispatcher folds
/// structurally, and the `signature` slot is typed `TypeExprRef`, so the operand
/// sub-dispatches to a type-side carrier and the bundle hands us the resolved
/// record. Each field becomes a keyword-less
/// `Argument`; the function registers no dispatch keyword (see
/// [`FnKind::Anonymous`]) and is reachable only through the value it returns.
pub fn body_record_schema<'a, 's>(
    scope: &'s Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let schema = match extract_ktype(&mut bundle, "signature") {
        Some(KType::Record(record)) => record,
        Some(other) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "anonymous FN signature must be a record schema `:{{…}}`, got `{}`",
                other.name(),
            ))));
        }
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "anonymous FN signature slot must be a record schema `:{…}`".to_string(),
            )));
        }
    };
    let elements: Vec<SignatureElement<'a>> = schema
        .iter()
        .map(|(name, ktype)| {
            SignatureElement::Argument(Argument {
                name: name.clone(),
                ktype: ktype.clone(),
            })
        })
        .collect();
    let param_names: Vec<String> = schema.keys().cloned().collect();

    let return_type_raw = match extract_return_type_raw(&mut bundle) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let body_expr = match bundle.extract_kexpression_or_shape_error("FN", "body") {
        Ok(e) => e,
        Err(e) => return err(e),
    };

    // `None` verdict context: the FUNCTOR-only return admissibility check is
    // skipped (an anonymous function is never a functor).
    let (return_type_state, _verdict) = match classify_return_type(
        return_type_raw,
        &param_names,
        scope,
        sched.current_lexical_chain(),
        None,
    ) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    // The schema is already resolved, so the parameter list never parks — only
    // the return type can defer. `classify` only routes on the return-type state
    // here (its `Done` params pass through on the synchronous arm and are
    // discarded on the Combine arm), so it gets an empty placeholder and the real
    // `elements` move into whichever arm runs: directly on the synchronous arm,
    // or through `prebuilt_elements` on the Combine arm (the anonymous form has
    // no keyword/arg signature expression to re-parse).
    match classify(return_type_state, ParamListResult::Done(Vec::new())) {
        FnPlan::Synchronous { return_type, .. } => finalize_fn_with_kind(
            scope,
            elements,
            return_type,
            body_expr,
            FnKind::Anonymous,
            bind_index,
        ),
        FnPlan::Combine(mut inputs) => {
            inputs.prebuilt_elements = Some(elements);
            defer_via_combine(
                scope,
                sched,
                crate::machine::model::ast::KExpression::new(Vec::new()),
                inputs,
                body_expr,
                FnKind::Anonymous,
                bind_index,
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Declared return is `KType::Any`: a function's structural type only exists
    // once its signature is known. The constructed `KObject::KFunction` projects
    // its full signature through `ktype()` at the call site.
    //
    // Two keyworded overloads cover the return-type carrier — `TypeExprRef` for a bare
    // `Type(_)` (`-> Number`) and `SigiledTypeExpr` for a `:(…)` / dotted form
    // (`-> Er.Type`, `-> :(Set WITH {…})`). `Future(KTypeValue(_))` post-Combine wakes
    // admit only against `TypeExprRef`. A third overload (below) carries the
    // anonymous `:{…}` record-schema signature.
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
        sig(
            KType::Any,
            vec![
                kw("FN"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        None,
        Some(binder_bucket),
        false,
    );
    // Lazy `:(...)` return carrier — a dotted/sigil return (`-> Er.Type`, `-> :(LIST OF T)`)
    // is a `SigiledTypeExpr`; the `:SigiledTypeExpr` slot captures it raw (more specific than
    // `:TypeExprRef`, so it wins) and `extract_return_type_raw` defers a param-referencing one
    // per-call instead of eager-sub-dispatching it to an unbound parameter.
    register_builtin_full(
        scope,
        "FN",
        sig(
            KType::Any,
            vec![
                kw("FN"),
                arg("signature", KType::KExpression),
                kw("->"),
                arg("return_type", KType::SigiledTypeExpr),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body,
        None,
        Some(binder_bucket),
        false,
    );
    // Anonymous overload: a `:{…}` record-schema operand is a `RecordType` part,
    // which the two `KExpression`-signature overloads above reject and only this
    // `TypeExprRef`-signature overload admits (it sub-dispatches to a resolved
    // `KType::Record`). Selection is unambiguous by operand part-kind, so it
    // needs no `binder_bucket` park-guard.
    register_builtin_full(
        scope,
        "FN",
        sig(
            KType::Any,
            vec![
                kw("FN"),
                arg("signature", KType::OfKind(KKind::Proper)),
                kw("->"),
                arg("return_type", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("body", KType::KExpression),
            ],
        ),
        body_record_schema,
        None,
        None,
        false,
    );
}

#[cfg(test)]
mod tests;

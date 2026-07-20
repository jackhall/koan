pub(crate) mod finalize;
mod param_refs;
pub(crate) mod return_type;
pub(crate) mod signature;

use crate::machine::model::Elaborator;
use crate::machine::model::KKind;
use crate::machine::model::TypeNode;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, KType, SignatureElement};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

use finalize::{classify, finalize_fn_with_kind, fn_action, FnKind, FnPlan, ParamListResult};
use return_type::classify_return_type;
use signature::ParamListOutcome;

pub(crate) use signature::binder_bucket;

/// Shared FN elaboration: extract the `signature` / return / `body` slots from
/// `BodyCtx::args`, collect param names, classify the return type, parse the param
/// list, and route to [`finalize_fn_with_kind`] (synchronous, via `Action::Done`) or
/// [`finalize::defer`] (dep-finish). `kind` selects how the finalized function is
/// wired into the scope; `builtin` (`"FN"`) names the surface in slot errors.
pub(crate) fn build_fn_like<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
    builtin: &str,
    kind: FnKind,
) -> crate::machine::Action<'a> {
    use crate::machine::{require_kexpression, Action};
    use finalize::defer;
    use return_type::extract_return_type_raw;

    let signature_expr = crate::try_action!(require_kexpression(ctx.args, builtin, "signature"));
    let return_type_raw = crate::try_action!(extract_return_type_raw(ctx.args));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, builtin, "body"));
    let param_names = signature::collect_param_names_from_signature(&signature_expr);
    let mut elaborator = Elaborator::new(ctx.scope).with_chain(ctx.chain.clone());
    let return_type_state = crate::try_action!(classify_return_type(
        return_type_raw,
        &param_names,
        ctx.scope,
        ctx.chain.clone(),
        "FN return-type slot",
        ctx.types,
    ));
    let params = match signature::parse_fn_param_list(&signature_expr, &mut elaborator, ctx.types) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg))))
        }
        ParamListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => ParamListResult::Pending {
            park_producers,
            sub_dispatches,
        },
    };
    let bind_index = ctx.bind_index();
    match classify(return_type_state, params) {
        FnPlan::Synchronous {
            elements,
            return_type,
        } => fn_action(finalize_fn_with_kind(
            ctx.scope,
            elements,
            return_type,
            body_expr,
            kind,
            bind_index,
            ctx.types,
        )),
        FnPlan::Deferred(inputs) => defer(signature_expr, inputs, body_expr, kind, bind_index),
    }
}

/// Keyworded FN body: the parenthesized `(<signature>)` form, which registers
/// under its lead keyword. At least one `Keyword` is required — an all-Argument
/// signature has no fast-lane shape to key on (every keyword-free expression
/// routes through `BareIdentifier` / `BareTypeLeaf` / `LiteralPassThrough` /
/// `TypeCall` / `FunctionValueCall` / `SigiledTypeExpr`), so the dispatcher needs
/// a fixed token. The keyword-less `FN :{…}` record-schema form is
/// [`body_record_schema`].
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    build_fn_like(ctx, "FN", FnKind::Function)
}

/// `-> <identifier>` — a return slot naming a value. Always errors: the slot names a type, and the
/// value it most often names is a module-valued parameter, whose type is `:(TYPE OF er)`.
pub fn body_value_named_return<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::{require_identifier_name, Action};

    let name = crate::try_action!(require_identifier_name(
        ctx.args,
        "return_type",
        "FN",
        ctx.types
    ));
    Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
        "FN return-type slot names a type, but `{name}` is a value. For the type of a value — a \
         module-valued parameter, say — write `-> :(TYPE OF {name})`"
    )))))
}

/// Anonymous-FN body: `FN :{<record schema>} -> ReturnType = (<body>)`.
///
/// The record-schema sigil `:{…}` resolves to a record-type `KType` before this
/// fires — it is a first-class `ExpressionPart::RecordType` the dispatcher folds
/// structurally, and the `signature` slot is typed `ProperType`, so the operand
/// sub-dispatches to a type-side carrier and the args record hands us the
/// resolved record. Each field becomes a keyword-less `Argument`; the function
/// registers no dispatch keyword (see [`FnKind::Anonymous`]) and is reachable
/// only through the value it returns.
pub fn body_record_schema<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_type, require_kexpression, Action};
    use finalize::defer;
    use return_type::extract_return_type_raw;

    let schema = match arg_type(ctx.args, "signature") {
        Some(kt) => match ctx.types.node(*kt) {
            TypeNode::Record { fields } => fields,
            _ => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "anonymous FN signature must be a record schema `:{{…}}`, got `{}`",
                    kt.name(ctx.types),
                )))))
            }
        },
        None => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "anonymous FN signature slot must be a record schema `:{…}`".to_string(),
            ))))
        }
    };
    let elements: Vec<SignatureElement> = schema
        .iter()
        .map(|(name, ktype)| {
            SignatureElement::Argument(Argument {
                name: name.clone(),
                ktype: *ktype,
            })
        })
        .collect();
    let param_names: Vec<String> = schema.keys().cloned().collect();
    let return_type_raw = crate::try_action!(extract_return_type_raw(ctx.args));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "FN", "body"));
    let return_type_state = crate::try_action!(classify_return_type(
        return_type_raw,
        &param_names,
        ctx.scope,
        ctx.chain.clone(),
        "FN return-type slot",
        ctx.types,
    ));
    let bind_index = ctx.bind_index();
    match classify(return_type_state, ParamListResult::Done(Vec::new())) {
        FnPlan::Synchronous { return_type, .. } => fn_action(finalize_fn_with_kind(
            ctx.scope,
            elements,
            return_type,
            body_expr,
            FnKind::Anonymous,
            bind_index,
            ctx.types,
        )),
        FnPlan::Deferred(mut inputs) => {
            inputs.prebuilt_elements = Some(elements);
            defer(
                crate::machine::model::KExpression::new(Vec::new()),
                inputs,
                body_expr,
                FnKind::Anonymous,
                bind_index,
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    // Declared return is `KType::ANY`: a function's structural type only exists
    // once its signature is known. The constructed `KObject::KFunction` projects
    // its full signature through `ktype()` at the call site.
    //
    // Two keyworded overloads cover the return-type carrier — `ProperType` for a bare
    // `Type(_)` (`-> Number`) and `SigiledTypeExpr` for a `:(…)` / dotted form
    // (`-> er.Carrier`, `-> :(Set WITH {…})`). A post-dep-finish `Spliced` cell carrying a type
    // admits only against `ProperType`. A third overload (below) carries the
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
    // `:ProperType`-return keyworded overload (`-> Number` / `-> er`).
    let typeexpr_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("FN"),
                arg("signature", KType::KEXPRESSION),
                kw("->"),
                arg("return_type", KType::of_kind(KKind::ProperType)),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    // Lazy `:(...)` return carrier — a dotted/sigil return (`-> er.Carrier`, `-> :(LIST OF Elem)`) is a
    // `SigiledTypeExpr`; the `:SigiledTypeExpr` slot captures it raw (more specific than
    // `:ProperType`, so it wins) and `extract_return_type_raw` defers a param-referencing one
    // per-call instead of eager-sub-dispatching it to an unbound parameter.
    let sigil_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("FN"),
                arg("signature", KType::KEXPRESSION),
                kw("->"),
                arg("return_type", KType::SIGILED_TYPE_EXPR),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    // Value-named return (`-> er`): a return slot names a *type*, and an Identifier names a value.
    // The overload exists only to diagnose — without it the shape falls through every FN overload
    // and reports "no matching function", which says nothing about the actual mistake. It is a
    // common one: a module-valued parameter is a value token, so the type it denotes is spelled
    // `:(TYPE OF er)`.
    let value_named_return_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("FN"),
                arg("signature", KType::KEXPRESSION),
                kw("->"),
                arg("return_type", KType::IDENTIFIER),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    // Anonymous overload: a `:{…}` record-schema operand is a `RecordType` part, which the two
    // `KExpression`-signature overloads above reject and only this `ProperType`-signature overload
    // admits (it sub-dispatches to a resolved record-type `KType`). Selection is unambiguous by operand
    // part-kind, so it needs no `binder_bucket` park-guard.
    let record_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("FN"),
                arg("signature", KType::of_kind(KKind::ProperType)),
                kw("->"),
                arg("return_type", KType::of_kind(KKind::ProperType)),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    use crate::builtins::register_builtin_full;
    register_builtin_full(
        scope,
        "FN",
        typeexpr_sig(),
        body,
        None,
        Some(binder_bucket),
        types,
    );
    register_builtin_full(
        scope,
        "FN",
        sigil_sig(),
        body,
        None,
        Some(binder_bucket),
        types,
    );
    register_builtin_full(
        scope,
        "FN",
        value_named_return_sig(),
        body_value_named_return,
        None,
        Some(binder_bucket),
        types,
    );
    register_builtin_full(
        scope,
        "FN",
        record_sig(),
        body_record_schema,
        None,
        None,
        types,
    );
}

#[cfg(test)]
mod tests;

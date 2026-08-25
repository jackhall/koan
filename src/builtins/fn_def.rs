pub(crate) mod finalize;
mod param_refs;
pub(crate) mod return_type;
pub(crate) mod signature;

use crate::machine::WriteGate;
use crate::machine::model::Elaborator;
use crate::machine::model::KKind;
use crate::machine::model::TypeNode;
use crate::machine::model::{Argument, BinderSymbol, KType, SignatureElement, Symbol, ValueSymbol};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

use crate::machine::BoundArgs;
use crate::machine::model::RunRegistries;
use finalize::{FnKind, FnPlan, ParamListResult, classify, finalize_fn_with_kind, fn_action};
use return_type::classify_return_type;
use signature::ParamListOutcome;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, name, return_type, signature } }

/// Shared FN elaboration: extract the `signature` / return / `body` slots from
/// `BodyCtx::args`, collect param names, classify the return type, parse the param
/// list, and route to [`finalize_fn_with_kind`] (synchronous, via `Action::Done`) or
/// [`finalize::defer`] (dep-finish). `kind` selects how the finalized function is
/// wired into the scope; `builtin` (`"FN"`) names the surface in slot errors.
pub(crate) fn build_fn_like<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
    builtin: &str,
    kind: FnKind,
) -> crate::machine::Action<'a> {
    use crate::machine::{Action, require_kexpression};
    use finalize::defer;
    use return_type::extract_return_type_raw;

    // The combined form binds a value name, and a SIG body declares members with `VAL` rather than
    // binding them. Guarded here, ahead of any deferral, so the synchronous and dep-finish paths
    // are covered once. A bare `FN` names no value binding, so it is unaffected.
    if let FnKind::Function {
        bound_name: Some(name),
    } = kind
        && ctx.scope.is_in_sig_body()
    {
        let name = crate::machine::model::render_label(name.symbol(), ctx.registries);
        return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write `(VAL {name}: <Type>)` \
                 instead of binding a function",
        )))));
    }
    let signature_expr =
        crate::try_action!(require_kexpression(ctx.args, builtin, &SLOTS.signature));
    let return_type_raw = crate::try_action!(extract_return_type_raw(ctx.args));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, builtin, &SLOTS.body));
    let param_names = signature::collect_param_names_from_signature(&signature_expr);
    let mut elaborator = Elaborator::new(ctx.scope).with_chain(ctx.chain.clone());
    let return_type_state = crate::try_action!(classify_return_type(
        return_type_raw,
        &param_names,
        ctx.scope,
        ctx.chain.clone(),
        "FN return-type slot",
        ctx.registries,
    ));
    let params = match signature::parse_fn_param_list(
        &signature_expr,
        &mut elaborator,
        ctx.registries,
        None,
    ) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(msg) => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(msg))));
        }
        ParamListOutcome::Pending {
            awaited_producers,
            sub_dispatches,
        } => ParamListResult::Pending {
            awaited_producers,
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
            ctx.registries,
        )),
        FnPlan::Deferred(inputs) => defer(
            ctx.scope,
            signature_expr,
            inputs,
            body_expr,
            kind,
            bind_index,
        ),
    }
}

/// Keyworded FN body: the parenthesized `(<signature>)` form, which registers
/// under its lead keyword. At least one `Keyword` is required — an all-Argument
/// signature has no fast-lane shape to key on (every keyword-free expression
/// routes through `BareIdentifier` / `BareTypeLeaf` / `LiteralPassThrough` /
/// `TypeCall` / `FunctionValueCall` / `SigiledTypeExpr`), so the dispatcher needs
/// a fixed token. The keyword-less `FN :{…}` record-schema form is
/// [`body_record_schema`].
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    build_fn_like(ctx, "FN", FnKind::Function { bound_name: None })
}

/// The `name` slot of a combined `LET <name> = …` statement, as the symbol the parse minted —
/// exactly as plain `LET` reads it. The slot is typed `IDENTIFIER`, so any other shape reaches a
/// sibling overload rather than this read.
pub(super) fn combined_bound_name(args: BoundArgs<'_, '_>) -> Result<ValueSymbol, KError> {
    args.identifier(&SLOTS.name)
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("name".to_string())))
}

/// `LET <name> = FN <signature> -> <return> = (<body>)` — one statement whose single binder
/// installs both channels: the value name and the signature's dispatch bucket. The bound value and
/// the registered overload are the same `KFunction` (see [`finalize_fn_with_kind`]).
pub fn body_let_combined<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let name = crate::try_action!(combined_bound_name(ctx.args));
    build_fn_like(
        ctx,
        "FN",
        FnKind::Function {
            bound_name: Some(name),
        },
    )
}

/// `LET <Name> = FN …` — a Type-classified binder over a function. A function is a value, so it
/// binds under a value-classified identifier; without this overload the shape is a bare dispatch
/// miss that says nothing about the actual mistake.
pub fn body_let_combined_type_named<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let name = match ctx.args.unresolved_type(&SLOTS.name) {
        Some(te) => crate::machine::model::render_label(te.symbol(), ctx.registries),
        None => match ctx.args.ktype(&SLOTS.name) {
            Some(kt) => kt.name(ctx.registries),
            None => return Action::done(Err(KError::new(KErrorKind::MissingArg("name".into())))),
        },
    };
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "LET binder `{name}` is Type-classified but the bound value is a function (a value); \
         rebind under a value-classified identifier instead (snake_case, e.g. `{suggestion}`)",
        suggestion = crate::builtins::let_binding::snake_case_identifier(&name),
    )))))
}

/// `-> <identifier>` — a return slot naming a value. Always errors: the slot names a type, and the
/// value it most often names is a module-valued parameter, whose type is `:(TYPE OF er)`.
pub fn body_value_named_return<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::{Action, require_identifier_name};

    let name = crate::try_action!(require_identifier_name(
        ctx.args,
        &SLOTS.return_type,
        "FN",
        ctx.registries
    ));
    let name = crate::machine::model::render_label(name.symbol(), ctx.registries);
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
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
pub fn body_record_schema<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::{Action, require_kexpression};
    use finalize::defer;
    use return_type::extract_return_type_raw;

    let schema = match ctx.args.ktype(&SLOTS.signature) {
        Some(kt) => match ctx.types().node(kt) {
            TypeNode::Record { fields } => fields,
            _ => {
                return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "anonymous FN signature must be a record schema `:{{…}}`, got `{}`",
                    kt.name(ctx.registries),
                )))));
            }
        },
        None => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(
                "anonymous FN signature slot must be a record schema `:{…}`".to_string(),
            ))));
        }
    };
    // The schema's field labels are bare symbols, so each parameter's binding class comes from
    // resolving its text through the run's interner and classifying that — the one seam where a
    // signature's names arrive without their class alongside. The return-surface scan probes by
    // bare symbol bits, so it reads the schema's keys directly.
    let param_names: Vec<Symbol> = schema.keys().collect();
    let mut elements: Vec<SignatureElement> = Vec::with_capacity(schema.len());
    for (field, ktype) in schema.iter() {
        let Some(text) = ctx.registries.labels.resolve(field) else {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(
                "anonymous FN signature field has no recorded name".to_string(),
            ))));
        };
        let Some(name) = BinderSymbol::classify(&text) else {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "anonymous FN parameter `{text}` is a keyword token, which nothing binds to",
            )))));
        };
        elements.push(SignatureElement::Argument(Argument {
            name,
            ktype: *ktype,
        }));
    }
    let return_type_raw = crate::try_action!(extract_return_type_raw(ctx.args));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "FN", &SLOTS.body));
    let return_type_state = crate::try_action!(classify_return_type(
        return_type_raw,
        &param_names,
        ctx.scope,
        ctx.chain.clone(),
        "FN return-type slot",
        ctx.registries,
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
            ctx.registries,
        )),
        FnPlan::Deferred(mut inputs) => {
            inputs.prebuilt_elements = Some(elements);
            defer(
                ctx.scope,
                crate::machine::model::KExpression::new(ctx.scope.brand(), &[]),
                inputs,
                body_expr,
                FnKind::Anonymous,
                bind_index,
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
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
    // The keyworded overloads share a bucket key the spec table lists, so a named `FN` installs a
    // pending-overload *bucket* entry and a forward sibling reference parks on it. FN's spec-table
    // extractor is `Bucket`, not `Name` — sibling FN overloads share one bucket and each installs
    // its own per-bucket entry, and consumers park on the earliest-index visible entry. A
    // single-name install (LET / UNION / SIG / MODULE, via `Name` extractors) would Rebind on the
    // second sibling sharing a head keyword (two `PICK` overloads both claiming the name `PICK`),
    // collapsing the overload set — right for a one-name-to-one-value binder, wrong for an overload
    // family.
    //
    // The record-schema overload shares that key but installs nothing: `fn_def_binder_bucket` reads
    // the signature operand as a parenthesized expression, and a `:{…}` record part is not one, so
    // the extractor returns `None`. An anonymous `FN :{…}` therefore claims no bucket and stays
    // legal in a value position.
    // `:ProperType`-return keyworded overload (`-> Number` / `-> er`).
    let typeexpr_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "FN"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(
                    registries,
                    &SLOTS.return_type,
                    KType::of_kind(KKind::ProperType),
                ),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
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
                kw(registries, "FN"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, KType::SIGILED_TYPE_EXPR),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
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
                kw(registries, "FN"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, KType::IDENTIFIER),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    // Anonymous overload: a `:{…}` record-schema operand is a `RecordType` part, which the two
    // `KExpression`-signature overloads above reject and only this `ProperType`-signature overload
    // admits (it sub-dispatches to a resolved record-type `KType`). Selection is unambiguous by operand
    // part-kind, so it needs no bucket park-guard.
    let record_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "FN"),
                arg(
                    registries,
                    &SLOTS.signature,
                    KType::of_kind(KKind::ProperType),
                ),
                kw(registries, "->"),
                arg(
                    registries,
                    &SLOTS.return_type,
                    KType::of_kind(KKind::ProperType),
                ),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    // The combined statement form: `LET <name> = FN <signature> -> <Return> = (<body>)`. One
    // statement, one binder, both install channels — the value name and the signature's dispatch
    // bucket. Full-bucket-key matching keeps `[LET, Slot, =, FN, Slot, ->, Slot, =, Slot]` disjoint
    // from plain `LET` and bare `FN`, so no overload of either is shadowed. The return-carrier and
    // value-named-return splits mirror the bare form's, one twin each.
    let combined_sig = |name: KType, signature: KType, return_type: KType| {
        sig(
            KType::ANY,
            vec![
                kw(registries, "LET"),
                arg(registries, &SLOTS.name, name),
                kw(registries, "="),
                kw(registries, "FN"),
                arg(registries, &SLOTS.signature, signature),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, return_type),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    use crate::builtins::register_builtin;
    register_builtin(scope, "FN", typeexpr_sig(), body, registries, gate);
    register_builtin(scope, "FN", sigil_sig(), body, registries, gate);
    register_builtin(
        scope,
        "FN",
        value_named_return_sig(),
        body_value_named_return,
        registries,
        gate,
    );
    register_builtin(
        scope,
        "FN",
        record_sig(),
        body_record_schema,
        registries,
        gate,
    );
    for return_type in [KType::of_kind(KKind::ProperType), KType::SIGILED_TYPE_EXPR] {
        register_builtin(
            scope,
            "LET",
            combined_sig(KType::IDENTIFIER, KType::KEXPRESSION, return_type),
            body_let_combined,
            registries,
            gate,
        );
    }
    register_builtin(
        scope,
        "LET",
        combined_sig(KType::IDENTIFIER, KType::KEXPRESSION, KType::IDENTIFIER),
        body_value_named_return,
        registries,
        gate,
    );
    // A diagnostic overload — it always errors, so it installs nothing: a function is a value, and
    // this names the shape that binds one under a Type-classified name.
    //
    // There is deliberately no combined overload for the anonymous `FN :{…}` signature. Its
    // `ProperType` signature slot would make the bucket's pick undecidable until that operand
    // sub-dispatched, and the re-resolve after an eager-subs round re-reads the statement's own
    // `name` token — which by then names this very node's placeholder, a self-cycle. The anonymous
    // form is not a binder anyway, so its flat spelling reports a plain dispatch miss and the
    // parenthesized value bind `LET f = (FN :{…} -> <Return> = (…))` stays the spelling.
    register_builtin(
        scope,
        "LET",
        combined_sig(
            KType::of_kind(KKind::ProperType),
            KType::KEXPRESSION,
            KType::of_kind(KKind::ProperType),
        ),
        body_let_combined_type_named,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests;

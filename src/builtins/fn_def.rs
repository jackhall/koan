pub(crate) mod finalize;
mod param_refs;
mod quantifiers;
pub(crate) mod return_type;
pub(crate) mod signature;

use crate::machine::WriteGate;
use crate::machine::model::Elaborator;
use crate::machine::model::KKind;
use crate::machine::model::TypeNode;
use crate::machine::model::{Argument, KType, SignatureElement};
use crate::machine::{KError, KErrorKind, Scope};
use crate::memory::BumpVec;
use crate::parse::{BinderSymbol, Symbol, ValueSymbol};

use super::{arg, arg_labeled, kw, sig};

use crate::machine::BoundArgs;
use crate::machine::model::RunRegistries;
use finalize::{
    FnKind, FnPlan, FnSurface, ParamListResult, Quantification, classify, finalize_fn_with_kind,
    fn_action,
};
use return_type::classify_return_type;
use signature::ParamListOutcome;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, name, quantifiers, return_type, signature } }

/// Shared FN elaboration: extract the `signature` / return / `body` slots from
/// `BodyCtx::args`, collect param names, classify the return type, parse the param
/// list, and route to [`finalize_fn_with_kind`] (synchronous, via `Action::Done`) or
/// [`finalize::defer`] (dep-finish). `kind` selects how the finalized function is
/// wired into the scope; `builtin` (`"EXPR"` or `"FN"`) names the surface in slot errors.
pub(crate) fn build_fn_like<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
    builtin: &str,
    surface: FnSurface<'a>,
) -> crate::machine::Action<'a> {
    use crate::machine::{Action, require_kexpression};
    use finalize::defer;
    use return_type::extract_return_type_raw;

    // Which definition surfaces a SIG body admits. A SIG declares members rather than defining
    // them, so a definition of either spelling is refused there and pointed at its declarator: the
    // combined form at `VAL`, the bare form at the bodyless head. Guarded here, ahead of any
    // deferral, so the synchronous and dep-finish paths are covered once.
    let FnSurface {
        kind,
        quantification,
    } = surface;
    let in_sig_body = ctx.scope.is_in_sig_body();
    match kind {
        FnKind::Function {
            bound_name: Some(name),
        } if in_sig_body => {
            let name = crate::machine::model::render_label(name.symbol(), ctx.registries);
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "inside a SIG body, value slots must use VAL — write `(VAL {name}: <Type>)` \
                 instead of binding a function",
            )))));
        }
        FnKind::Function { bound_name: None } if in_sig_body => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(
                "inside a SIG body, a keyworded member is declared rather than defined — drop \
                 the `= (<body>)` and write `(EXPR (<head>) -> <Return>)`"
                    .to_string(),
            ))));
        }
        _ => {}
    }
    let signature_expr =
        crate::try_action!(require_kexpression(ctx.args, builtin, &SLOTS.signature));
    // A bodyless head has no body slot to read; the empty expression stands in for one, and the
    // bodyless leg of the finalize never looks at it.
    let body_expr = match kind {
        FnKind::Shape { .. } => crate::parse::KExpression::new(ctx.scope.brand(), &[]),
        _ => crate::try_action!(require_kexpression(ctx.args, builtin, &SLOTS.body)),
    };
    let mut elaborator = Elaborator::new(quantification.scope).with_chain(ctx.chain.clone());
    // A definition's return slot captures raw, because it may name a parameter and has to survive
    // verbatim to the per-call boundary. A bodyless head's cannot: there is no call to elaborate it
    // at, so its slot is an ordinary kind expectation the lane resolves against the SIG body's own
    // scope — which is what lets `-> Carrier` read the signature's abstract member.
    let eager_return = matches!(kind, FnKind::Shape { .. }) && quantification.names.is_empty();
    let return_type_state = match eager_return {
        true => match ctx.args.ktype(&SLOTS.return_type) {
            Some(ret) => return_type::ReturnTypeState::Done(ret),
            None => {
                return Action::done(Err(KError::new(KErrorKind::MissingArg(
                    "return_type".to_string(),
                ))));
            }
        },
        false => {
            let return_type_raw =
                crate::try_action!(extract_return_type_raw(ctx.args, ctx.scope.brand()));
            let param_names =
                signature::collect_param_names_from_signature(&signature_expr, ctx.scratch);
            crate::try_action!(classify_return_type(
                return_type_raw,
                &param_names,
                quantification.scope,
                ctx.chain.clone(),
                "return-type slot",
                ctx.registries,
            ))
        }
    };
    let params = match signature::parse_fn_param_list(
        &signature_expr,
        &mut elaborator,
        ctx.registries,
        None,
        kind.wildcards(),
        ctx.scratch,
    ) {
        ParamListOutcome::Done(es) => ParamListResult::Done(es),
        ParamListOutcome::Err(error) => return Action::done(Err(error)),
        ParamListOutcome::Pending {
            awaited_producers,
            sub_dispatches,
        } => ParamListResult::Pending {
            awaited_producers,
            sub_dispatches,
        },
    };
    let bind_index = ctx.bind_index();
    match classify(return_type_state, params, ctx.scratch) {
        FnPlan::Synchronous {
            elements,
            return_type,
        } => fn_action(
            ctx.scratch,
            finalize_fn_with_kind(
                ctx.scope,
                &elements,
                return_type,
                body_expr,
                surface,
                bind_index,
                ctx.registries,
            ),
        ),
        FnPlan::Deferred(inputs) => defer(
            ctx.scope,
            signature_expr,
            inputs,
            body_expr,
            surface,
            bind_index,
        ),
    }
}

/// `EXPR (<head>) -> <Return> = (<body>)` — the definition, which registers under its head's
/// bucket key. At least one `Keyword` is required — an all-Argument head has no fast-lane shape to
/// key on (every keyword-free expression routes through `BareIdentifier` / `BareTypeLeaf` /
/// `LiteralPassThrough` / `TypeCall` / `FunctionValueCall` / `SigiledTypeExpr`), so the dispatcher
/// needs a fixed token. The keyword-free callable is the lambda `FN :{…}`, whose body is
/// [`body_record_schema`].
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Function { bound_name: None },
            quantification: Quantification::none(ctx.scope),
        },
    )
}

/// `EXPR (<head>) -> <Return>` — the bodyless head, whose carrier is the head's expression shape as
/// a type value. Bare inside a SIG body it is also the declaration of a keyworded (dispatch-bucket)
/// member of the signature under construction; under `:(…)` — the type sigil the enclosing
/// expression stamps — it is the type value and nothing else, and outside a SIG body there is no
/// signature to record into either way. Same head shape and same parse path as the definition form,
/// so a declaration and the definition that satisfies it derive one shape.
pub fn body_shape<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Shape {
                declare: declares_here(ctx),
            },
            quantification: Quantification::none(ctx.scope),
        },
    )
}

/// Whether a bodyless `EXPR` head standing here also declares: bare inside a SIG body it records a
/// keyworded member of the signature under construction, and under `:(…)` — the type-context stamp
/// the enclosing expression leaves — it is the type value alone.
fn declares_here(ctx: &crate::machine::BodyCtx<'_, '_, '_>) -> bool {
    ctx.scope.is_in_sig_body() && !ctx.under_type_sigil
}

/// The `FOR ALL (<names>)` group of a quantified form, read and bound before the head elaborates.
fn quantification_of<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> Result<Quantification<'a>, KError> {
    let group = crate::machine::require_kexpression(ctx.args, "EXPR", &SLOTS.quantifiers)?;
    quantifiers::read_quantification(ctx, &group)
}

/// `EXPR FOR ALL (<names>) (<head>) -> <Return>` — the quantified bodyless head. The group binds
/// first, so the head's slot types and its return lower against it.
pub fn body_quantified_shape<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let quantification = crate::try_action!(quantification_of(ctx));
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Shape {
                declare: declares_here(ctx),
            },
            quantification,
        },
    )
}

/// `EXPR FOR ALL (<names>) (<head>) -> <Return> = (<body>)` — the quantified definition.
pub fn body_quantified<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let quantification = crate::try_action!(quantification_of(ctx));
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Function { bound_name: None },
            quantification,
        },
    )
}

/// `LET <name> = FN EXPR FOR ALL (<names>) (<head>) -> <Return> = (<body>)` — the quantified
/// combined statement.
pub fn body_quantified_let_combined<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let name = crate::try_action!(combined_bound_name(ctx.args));
    let quantification = crate::try_action!(quantification_of(ctx));
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Function {
                bound_name: Some(name),
            },
            quantification,
        },
    )
}

/// The `name` slot of a combined `LET <name> = …` statement, as the symbol the parse minted —
/// exactly as plain `LET` reads it. The slot is typed `IDENTIFIER`, so any other shape reaches a
/// sibling overload rather than this read.
pub(super) fn combined_bound_name(args: BoundArgs<'_, '_>) -> Result<ValueSymbol, KError> {
    args.identifier(&SLOTS.name)
        .ok_or_else(|| KError::new(KErrorKind::MissingArg("name".to_string())))
}

/// `LET <name> = FN EXPR (<head>) -> <Return> = (<body>)` — one statement whose single binder
/// installs both channels: the value name, lambda-typed, and the head's dispatch bucket. The bound
/// value and the registered overload are the same `KFunction` (see [`finalize_fn_with_kind`]).
pub fn body_let_combined<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    let name = crate::try_action!(combined_bound_name(ctx.args));
    build_fn_like(
        ctx,
        "EXPR",
        FnSurface {
            kind: FnKind::Function {
                bound_name: Some(name),
            },
            quantification: Quantification::none(ctx.scope),
        },
    )
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

    let Some(schema_kt) = ctx.args.ktype(&SLOTS.signature) else {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(
            "anonymous FN signature slot must be a record schema `:{…}`".to_string(),
        ))));
    };
    // The schema's keys are the classified names its own field-list parse minted, so each
    // parameter's binding class rides straight into its `Argument`. The return-surface scan probes
    // by bare symbol bits, so it reads the same keys down to their symbols. Both lists are derived
    // under the type-table borrow, so the schema is read in place rather than copied out. The names
    // die with this step, once the return-surface scan below has read them, so they stage on the
    // step scratch; the elements run outlives it whenever the return type defers — the wake closure
    // carries that one as `prebuilt_elements` — so it takes the frame region instead. Each list
    // gets one entry per schema field, so both reservations are exact.
    let read = ctx.types().with_node(schema_kt, |node| match node {
        TypeNode::Record { fields } => {
            let mut param_names: BumpVec<'a, Symbol> =
                BumpVec::with_capacity_in(fields.len(), ctx.scratch);
            param_names.extend(fields.keys().map(BinderSymbol::symbol));
            let mut elements: BumpVec<'a, SignatureElement> =
                BumpVec::with_capacity_in(fields.len(), ctx.scope.brand().allocator());
            elements.extend(
                fields
                    .iter()
                    .map(|(name, ktype)| SignatureElement::Argument(Argument::new(name, *ktype))),
            );
            Some((param_names, elements))
        }
        _ => None,
    });
    let Some((param_names, elements)) = read else {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
            "anonymous FN signature must be a record schema `:{{…}}`, got `{}`",
            schema_kt.display_name(ctx.registries),
        )))));
    };
    let return_type_raw = crate::try_action!(extract_return_type_raw(ctx.args, ctx.scope.brand()));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "FN", &SLOTS.body));
    let return_type_state = crate::try_action!(classify_return_type(
        return_type_raw,
        &param_names,
        ctx.scope,
        ctx.chain.clone(),
        "return-type slot",
        ctx.registries,
    ));
    let bind_index = ctx.bind_index();
    match classify(
        return_type_state,
        ParamListResult::Done(BumpVec::new_in(ctx.scratch)),
        ctx.scratch,
    ) {
        FnPlan::Synchronous { return_type, .. } => fn_action(
            ctx.scratch,
            finalize_fn_with_kind(
                ctx.scope,
                &elements,
                return_type,
                body_expr,
                FnSurface {
                    kind: FnKind::Anonymous,
                    quantification: Quantification::none(ctx.scope),
                },
                bind_index,
                ctx.registries,
            ),
        ),
        FnPlan::Deferred(mut inputs) => {
            inputs.prebuilt_elements = Some(elements);
            defer(
                ctx.scope,
                crate::parse::KExpression::new(ctx.scope.brand(), &[]),
                inputs,
                body_expr,
                FnSurface {
                    kind: FnKind::Anonymous,
                    quantification: Quantification::none(ctx.scope),
                },
                bind_index,
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    // Declared return is `KType::ANY`: a function's structural type only exists once its
    // signature is known. The constructed `KObject::KFunction` projects its full signature through
    // `ktype()` at the call site.
    //
    // Two families register here. `FN :{…} -> <Ret> = (<body>)` is the lambda: anonymous, reached
    // by name, its arguments a record of named fields. `EXPR` is the expression shape: keyworded,
    // reached by dispatch, its arguments positional. Every `EXPR` key carries that keyword, so the
    // two families are disjoint by construction and never compete for a pick.
    //
    // The `EXPR` keys share a bucket key the spec table lists, so a definition installs a
    // pending-overload *bucket* entry and a forward sibling reference parks on it. The spec-table
    // extractor is `Bucket`, not `Name` — sibling overloads share one bucket and each installs its
    // own per-bucket entry, and consumers park on the earliest-index visible entry. A single-name
    // install (LET / UNION / SIG / MODULE, via `Name` extractors) would Rebind on the second
    // sibling sharing a head keyword (two `PICK` overloads both claiming the name `PICK`),
    // collapsing the overload set — right for a one-name-to-one-value binder, wrong for an overload
    // family. The lambda claims no bucket at all: `fn_def_binder_bucket` reads the signature
    // operand as a parenthesized expression, and a `:{…}` record part is not one, so the extractor
    // returns `None` and an anonymous `FN :{…}` stays legal in a value position.
    //
    // The return-carrier union is shared by every definition surface below and with `OP`'s operand
    // / result slots: the slot is a union of the raw-capture members the return position admits — a
    // bare `Type` token (`-> Number`, `-> MyAlias`), a `:(…)` / dotted form (`-> er.Carrier`,
    // `-> :(Set WITH {…})`), and a `:{…}` record (`-> :{v :Number}`). Every member captures raw,
    // because a return type may name a parameter unbound in the defining scope and must survive
    // verbatim to the dispatch boundary. A value-named return (`-> er`) is no member of it: the
    // shape is a mistake, and its targeted message comes from the dispatch-miss diagnosis table
    // rather than from an always-erroring overload sitting in this bucket.
    let return_union = return_type::type_carrier_union(registries);
    // The lambda: a `:{…}` record-schema operand is a `RecordType` part, which every
    // `KExpression`-signature overload rejects and only this `ProperType`-signature one admits.
    // The signature slot stays a pure kind expectation — a `:{…}` sub-dispatches to a
    // resolved record-type `KType`, and a bare `Type` token naming a record alias auto-wraps to
    // one — so an alias and a literal reach the same body read. Selection is unambiguous by operand
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
                arg(registries, &SLOTS.return_type, return_union),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    // The expression-shape surfaces. `EXPR` marks the callable whose arguments are positional and
    // reached by dispatch — so the definition and the bodyless head both spell it, and the combined
    // statement spells `FN EXPR` because the value channel it also binds is a lambda-typed name.
    let shape_definition_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "EXPR"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, return_union),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    // The bodyless head. Its return slot is an ordinary kind expectation rather than the
    // definition's raw-carrier union: a bodyless head's return resolves once, where it is written.
    let shape_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "EXPR"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg_labeled(
                    registries,
                    &SLOTS.return_type,
                    KType::of_kind(KKind::AnyType),
                    "expression-shape return type",
                ),
            ],
        )
    };
    let shape_combined_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "LET"),
                arg(registries, &SLOTS.name, KType::IDENTIFIER),
                kw(registries, "="),
                kw(registries, "FN"),
                kw(registries, "EXPR"),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, return_union),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    // The quantified twins. `FOR ALL (<names>)` is a group of its own, captured raw beside the
    // head — its tokens name nothing until this form binds them — so each quantified key sits in
    // its own bucket beside its unquantified twin, as `OP … -> R` sits beside `OP …`. The bodyless
    // twin's return takes the raw carrier union rather than a kind expectation, because a
    // quantified return names the group and so resolves against the group's own scope.
    let quantified_head = |extra: Vec<SignatureElement>| {
        let mut elements = vec![
            kw(registries, "EXPR"),
            kw(registries, "FOR"),
            kw(registries, "ALL"),
            arg(registries, &SLOTS.quantifiers, KType::KEXPRESSION),
            arg(registries, &SLOTS.signature, KType::KEXPRESSION),
            kw(registries, "->"),
            arg(registries, &SLOTS.return_type, return_union),
        ];
        elements.extend(extra);
        sig(KType::ANY, elements)
    };
    let quantified_shape_sig = || quantified_head(vec![]);
    let quantified_definition_sig = || {
        quantified_head(vec![
            kw(registries, "="),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ])
    };
    let quantified_combined_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "LET"),
                arg(registries, &SLOTS.name, KType::IDENTIFIER),
                kw(registries, "="),
                kw(registries, "FN"),
                kw(registries, "EXPR"),
                kw(registries, "FOR"),
                kw(registries, "ALL"),
                arg(registries, &SLOTS.quantifiers, KType::KEXPRESSION),
                arg(registries, &SLOTS.signature, KType::KEXPRESSION),
                kw(registries, "->"),
                arg(registries, &SLOTS.return_type, return_union),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    use crate::builtins::register_builtin;
    register_builtin(scope, record_sig(), body_record_schema, registries, gate);
    register_builtin(scope, shape_definition_sig(), body, registries, gate);
    register_builtin(scope, shape_sig(), body_shape, registries, gate);
    register_builtin(
        scope,
        shape_combined_sig(),
        body_let_combined,
        registries,
        gate,
    );
    register_builtin(
        scope,
        quantified_definition_sig(),
        body_quantified,
        registries,
        gate,
    );
    register_builtin(
        scope,
        quantified_shape_sig(),
        body_quantified_shape,
        registries,
        gate,
    );
    register_builtin(
        scope,
        quantified_combined_sig(),
        body_quantified_let_combined,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests;

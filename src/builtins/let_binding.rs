use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::core::bindings::{TypeWritePolicy, WriteOp};
use crate::machine::model::TypeNode;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::Carried;
use crate::machine::model::RunRegistries;
use crate::machine::model::{BindKind, BinderSymbol, display_label};

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { name, value } }

/// `LET <name> = <value:Any>` — deep-clones the bound value into the region and inserts it
/// under `name`. The `name` slot is a `NameToken`, so the binder arrives as the classified
/// symbol the parse minted, whichever class it is, and the two channels part on that class alone.
/// Reads its args from the `BodyCtx::args` record, writes the binding directly on `ctx.scope`
/// (interior-mutable), and returns the bound carrier as `Action::Done`.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    use crate::machine::model::Held;

    let done_err = |e: KError| Action::done(Err(e));
    let bind_index = ctx.bind_index();
    let rhs = match ctx.args.held(&SLOTS.value) {
        Some(v) => v,
        None => return done_err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let mut type_for_types_map: Option<KType> = None;
    // The binder, carried straight off the `NameToken` slot: the parse classified and interned the
    // token, so the symbol here is the one it minted and its class is the one the parser assigned.
    // Each diagnostic that quotes the name renders it on its own arm, so a binding that succeeds
    // reads no label text.
    let binder = match ctx.args.name(&SLOTS.name) {
        None => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
        Some(BinderSymbol::Value(v)) => {
            // A type-language carrier under a value-classified name is a cross-kind error. A module
            // is *not* one: it is a value, and a value-classified name is exactly where it belongs.
            let type_kind = match rhs {
                Held::Type(kt)
                    if ctx
                        .types()
                        .with_node(*kt, |n| matches!(n, TypeNode::Signature { .. })) =>
                {
                    Some("signature")
                }
                Held::Type(_) | Held::UnresolvedType(_) => Some("type"),
                Held::Object(_) => None,
                // The RHS slot is `:Any`, which admits no raw name part.
                Held::Name(_) => {
                    unreachable!("LET's bound-value slot never captures a name")
                }
            };
            if let Some(kind) = type_kind {
                let spelling = ctx.registries.labels.render(v.symbol());
                let example = match capitalize_identifier(&spelling) {
                    Some(suggestion) => format!(", e.g. `{suggestion}`"),
                    None => String::new(),
                };
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "LET binder `{spelling}` is value-classified but the bound value is a \
                     {kind} (a type-language carrier); rebind under a Type-classified \
                     identifier instead (uppercase-leading plus at least one lowercase \
                     letter{example})",
                ))));
            }
            BinderSymbol::Value(v)
        }
        Some(BinderSymbol::Type(type_name)) => {
            match rhs {
                Held::Type(kt) => type_for_types_map = Some(*kt),
                // The `Any` RHS slot is auto-wrapped by dispatch into a resolved carrier, so a
                // name that reaches here unlowered names nothing.
                Held::UnresolvedType(te) => {
                    return done_err(KError::new(KErrorKind::UnboundName(
                        crate::machine::model::render_label(te.symbol(), ctx.registries),
                    )));
                }
                // A module is a value, and the Type-token namespace names things that type a field.
                // `LET view = (m :| S)` is the wrong spelling for a module binding, whatever the RHS
                // produced it (an ascription view, a functor call) — respell it snake_case.
                // The RHS slot is `:Any`, which admits no raw name part.
                Held::Name(_) => {
                    unreachable!("LET's bound-value slot never captures a name")
                }
                Held::Object(KObject::Module(_)) => {
                    let spelling =
                        crate::machine::model::render_label(type_name.symbol(), ctx.registries);
                    return done_err(KError::new(KErrorKind::ShapeError(format!(
                        "LET binder `{spelling}` is Type-classified but the bound value is a \
                         module (a value); rebind under a value-classified identifier instead \
                         (snake_case, e.g. `{suggestion}`)",
                        suggestion = snake_case_identifier(&spelling),
                    ))));
                }
                Held::Object(o) => {
                    return done_err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: crate::machine::model::render_label(
                            type_name.symbol(),
                            ctx.registries,
                        ),
                        got: o.ktype().name(ctx.registries),
                    }));
                }
            }
            BinderSymbol::Type(type_name)
        }
    };
    if binder.bind_kind() == BindKind::Value && ctx.scope.is_in_sig_body() {
        let name = display_label(binder.symbol(), ctx.registries);
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write \
             `(VAL {name}: <Type>)` instead of `(LET {name} = <example-value>)`",
        ))));
    }
    if let Some(kt) = type_for_types_map {
        // The type channel needs no seam of its own: the parser classified the token Type-side
        // when it minted the symbol, and past that point the key types make a crossing
        // unrepresentable.
        let BinderSymbol::Type(name) = binder else {
            unreachable!("the type-channel route is reached only under a Type-classified binder")
        };
        // The handle names the same interned type in every region — `kt` is already this binder's
        // copy out of the RHS envelope — so the terminal witnesses it directly and the `types`
        // write rides the outcome.
        let carrier = ctx.scope.resident(Carried::Type(kt));
        Action::done(Ok(StepCarried::born(carrier))).with_effect(
            ctx.scratch,
            WriteOp::Type {
                name,
                kt,
                site: ctx.declaration_site(),
                policy: TypeWritePolicy::Insert,
                builtin_shadow_guard: true,
            },
        )
    } else {
        // The value channel needs no seam of its own: only a value-classified binder reaches here,
        // and its symbol was classified value-side at the parse.
        let BinderSymbol::Value(binder) = binder else {
            unreachable!("the value-channel route is reached only under a value-classified binder")
        };
        let value = rhs
            .as_object()
            .expect("value-route LET RHS is the Object arm");
        // An empty container has no element type to infer. The check reads the source value; a
        // deep-clone into the region preserves the unstamped shape, so it settles here before the
        // fused bind installs anything.
        if value.is_unstamped_empty_container() {
            let name = display_label(binder.symbol(), ctx.registries);
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "empty container bound to `{name}` has no element type to infer; \
                 annotate the value's type (e.g. via a typed FN return) or use a \
                 non-empty literal",
            ))));
        }
        // Fused mint + copy + seal. A delivered RHS carrier derives the copy's stored reach in copied
        // mode — the deep-clone lands in this scope's own region, so a residence-only host is dropped
        // (`adopted_reach_of`, the same split the parameter and MATCH `it` binds apply) — and copies
        // the value in under it. A carrier-less region-pure RHS is placed through the shape-split
        // pure door, which seals it resident: the value borrows only this region, so its
        // description names no member. Either
        // returns the sealed value, from which the
        // terminal witnesses the bound value in place — the same reach-aware wrapper a later read
        // uses — while the table write rides the outcome.
        let sealed = match ctx.args.carrier(&SLOTS.value) {
            Some(carrier) => ctx
                .scope
                .adopt_for_binding(carrier, |carried| Ok(carried.object())),
            None => ctx.scope.seal_pure_value(value),
        };
        let sealed = match sealed {
            Ok(sealed) => sealed,
            Err(e) => return done_err(e),
        };
        // The bound value's own reach rides the terminal carrier out of the step: the lift upgrades
        // the entry's exact description into owned pins, so the reached regions stay pinned across
        // transit (the delivery envelope) rather than being re-derived at the seal. The write takes
        // its own duplicate of the seal, so the finish never reads its own write back out.
        let write = WriteOp::Value {
            name: binder,
            index: bind_index,
            sealed: sealed.duplicate(),
        };
        Action::done(Ok(StepCarried::born_delivered(
            ctx.scope.lift_resident(sealed),
        )))
        .with_effect(ctx.scratch, write)
    }
}

/// Suggest a value-classified rewrite of a Type-classified binder name for the module guard:
/// `IntOrd` → `int_ord`. Each interior uppercase letter opens a new word (see
/// design/typing/tokens.md for the two token classes).
pub(crate) fn snake_case_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// A Type-classified rewrite of a value-classified binder name for the partition-guard
/// diagnostic, or [`None`] when capitalizing the name does not reach a Type token.
///
/// Checked against [`is_type_name`], the one classifier the parser tags a `Type` part by, so the
/// suggestion is always a spelling the writer can actually type: `t` capitalizes to `T`, which is
/// one uppercase letter with no lowercase and so classifies as neither keyword nor type name (see
/// [design/typing/tokens.md](../../design/typing/tokens.md)). A name that has no such rewrite is
/// reported with the rule alone.
fn capitalize_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut out = String::with_capacity(name.len());
    out.push(first.to_ascii_uppercase());
    out.extend(chars);
    crate::machine::model::labels::is_type_name(&out).then_some(out)
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let name_sig = || {
        sig(
            KType::ANY,
            vec![
                kw(registries, "LET"),
                arg(registries, &SLOTS.name, KType::NAME_TOKEN),
                kw(registries, "="),
                arg(registries, &SLOTS.value, KType::ANY),
            ],
        )
    };
    crate::builtins::register_builtin(scope, name_sig(), body, registries, gate);
}

#[cfg(test)]
mod tests;

use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::core::bindings::{TypeWritePolicy, WriteOp};
use crate::machine::model::KKind;
use crate::machine::model::TypeNode;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::Carried;

/// `LET <name> = <value:Any>` — deep-clones the bound value into the region and inserts it
/// under `name`. Two overloads share this body, differing only in the `name` slot's `KType`:
/// `Identifier` and `ProperType`. Same partition logic across both: reads its args from the
/// `BodyCtx::args` record, writes the binding directly on `ctx.scope` (interior-mutable), and
/// returns the bound carrier as `Action::Done`.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::model::Held;
    use crate::machine::{Action, arg_held, arg_object, arg_type, arg_unresolved_type};

    let done_err = |e: KError| Action::done(Err(e));
    let bind_index = ctx.bind_index();
    let rhs = match arg_held(ctx.args, "value") {
        Some(v) => v,
        None => return done_err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let mut type_for_types_map: Option<KType> = None;
    // Whether the binder name is Type-classified (`LET <Name> = …`) — the SIG-body VAL guard below
    // keys on it, independent of which map the RHS lands in.
    let mut type_classified_name = false;
    // The Type-classified `name` slot arrives either lowered (a builtin leaf name) or as the
    // unlowered surface name the bind seam leaves for the binder to own; both denote the binder.
    let type_name: Option<String> = match arg_unresolved_type(ctx.args, "name") {
        Some(te) => Some(te.render()),
        None => match arg_type(ctx.args, "name") {
            Some(name_kt)
                if matches!(
                    ctx.types.node(name_kt),
                    TypeNode::List { .. }
                        | TypeNode::Dict { .. }
                        | TypeNode::KFunction { .. }
                        | TypeNode::Sibling(_)
                ) =>
            {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    name_kt.render(ctx.types),
                ))));
            }
            Some(name_kt) => Some(name_kt.name(ctx.types)),
            None => None,
        },
    };
    let name = match (arg_object(ctx.args, "name"), type_name) {
        (Some(KObject::KString(s)), _) => {
            // A type-language carrier under a value-classified name is a cross-kind error. A module
            // is *not* one: it is a value, and a value-classified name is exactly where it belongs.
            let type_kind = match rhs {
                Held::Type(kt) if matches!(ctx.types.node(*kt), TypeNode::Signature { .. }) => {
                    Some("signature")
                }
                Held::Type(_) | Held::UnresolvedType(_) => Some("type"),
                Held::Object(_) => None,
            };
            if let Some(kind) = type_kind {
                return done_err(KError::new(KErrorKind::ShapeError(format!(
                    "LET binder `{name}` is value-classified but the bound value is a \
                     {kind} (a type-language carrier); rebind under a Type-classified \
                     identifier instead (uppercase-leading plus at least one lowercase \
                     letter, e.g. `{suggestion}`)",
                    name = s,
                    suggestion = capitalize_identifier(s),
                ))));
            }
            (*s).to_string()
        }
        (_, Some(resolved_name)) => {
            type_classified_name = true;
            match rhs {
                Held::Type(kt) => type_for_types_map = Some(*kt),
                // The `Any` RHS slot is auto-wrapped by dispatch into a resolved carrier, so a
                // name that reaches here unlowered names nothing.
                Held::UnresolvedType(te) => {
                    return done_err(KError::new(KErrorKind::UnboundName(te.render())));
                }
                // A module is a value, and the Type-token namespace names things that type a field.
                // `LET view = (m :| S)` is the wrong spelling for a module binding, whatever the RHS
                // produced it (an ascription view, a functor call) — respell it snake_case.
                Held::Object(KObject::Module(_)) => {
                    return done_err(KError::new(KErrorKind::ShapeError(format!(
                        "LET binder `{resolved_name}` is Type-classified but the bound value is a \
                         module (a value); rebind under a value-classified identifier instead \
                         (snake_case, e.g. `{suggestion}`)",
                        suggestion = snake_case_identifier(&resolved_name),
                    ))));
                }
                Held::Object(o) => {
                    return done_err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: o.ktype().name(ctx.types),
                    }));
                }
            }
            resolved_name
        }
        (Some(other), None) => {
            return done_err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or ProperType".to_string(),
                got: other.ktype().name(ctx.types),
            }));
        }
        (None, None) => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    if !type_classified_name && ctx.scope.is_in_sig_body() {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write \
             `(VAL {name}: <Type>)` instead of `(LET {name} = <example-value>)`",
        ))));
    }
    if let Some(kt) = type_for_types_map {
        // The handle names the same interned type in every region — `kt` is already this binder's
        // copy out of the RHS envelope — so the terminal witnesses it directly and the `types`
        // write rides the outcome.
        let carrier = ctx.scope.resident(Carried::Type(kt));
        Action::done(Ok(StepCarried::born(carrier))).with_effect(WriteOp::Type {
            name,
            kt,
            site: ctx.declaration_site(),
            policy: TypeWritePolicy::Insert,
            builtin_shadow_guard: true,
        })
    } else {
        let value = rhs
            .as_object()
            .expect("value-route LET RHS is the Object arm");
        // An empty container has no element type to infer. The check reads the source value; a
        // deep-clone into the region preserves the unstamped shape, so it settles here before the
        // fused bind installs anything.
        if value.is_unstamped_empty_container() {
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
        let sealed = match ctx.arg_carrier("value") {
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
            name,
            index: bind_index,
            sealed: sealed.duplicate(),
        };
        Action::done(Ok(StepCarried::born_delivered(
            ctx.scope.lift_resident(sealed),
        )))
        .with_effect(write)
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

/// Suggest a Type-classified rewrite of a value-classified binder name for the
/// partition-guard diagnostic. Falls back to a synthetic `M` prefix if the name
/// starts with a non-alphabetic character where simple capitalization wouldn't
/// yield a Type-shape token (see design/typing/tokens.md).
fn capitalize_identifier(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            let mut out = String::with_capacity(name.len());
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        }
        _ => format!("M{name}"),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let identifier_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("LET"),
                arg("name", KType::IDENTIFIER),
                kw("="),
                arg("value", KType::ANY),
            ],
        )
    };
    let type_sig = || {
        sig(
            KType::ANY,
            vec![
                kw("LET"),
                arg("name", KType::of_kind(KKind::ProperType)),
                kw("="),
                arg("value", KType::ANY),
            ],
        )
    };
    crate::builtins::register_builtin(scope, "LET", identifier_sig(), body, types, gate);
    crate::builtins::register_builtin(scope, "LET", type_sig(), body, types, gate);
}

#[cfg(test)]
mod tests;

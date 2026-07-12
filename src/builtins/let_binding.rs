use crate::machine::execute::StepCarried;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `LET <name> = <value:Any>` — deep-clones the bound value into the region and inserts it
/// under `name`. Two overloads share this body, differing only in the `name` slot's `KType`:
/// `Identifier` and `ProperType`. Same partition logic across both: reads its args from the
/// `BodyCtx::args` record, writes the binding directly on `ctx.scope` (interior-mutable), and
/// returns the bound carrier as `Action::Done`.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_held, arg_object, arg_type, Action};
    use crate::machine::model::values::Held;

    let done_err = |e: KError| Action::Done(Err(e));
    let bind_index = ctx.bind_index();
    let rhs = match arg_held(ctx.args, "value") {
        Some(v) => v,
        None => return done_err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let mut type_for_types_map: Option<KType<'a>> = None;
    let name = match (arg_object(ctx.args, "name"), arg_type(ctx.args, "name")) {
        (Some(KObject::KString(s)), _) => {
            // A type-language carrier under a value-classified name is a cross-kind error. A module
            // rides the Object arm (`KObject::Module`) but is still type-language, so it keys the same
            // diagnostic as a `Held::Type` module identity.
            let type_kind = match rhs {
                Held::Object(KObject::Module(_)) => Some("module"),
                Held::Type(KType::Module { .. }) => Some("module"),
                Held::Type(KType::Signature { .. }) => Some("signature"),
                Held::Type(_) => Some("type"),
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
            s.clone()
        }
        (_, Some(name_kt)) => {
            let resolved_name = match name_kt {
                KType::Unresolved(te) => te.render(),
                KType::List(_)
                | KType::Dict(_, _)
                | KType::KFunction { .. }
                | KType::KFunctor { .. }
                | KType::SetLocal(_)
                | KType::RecursiveRef(_) => {
                    return done_err(KError::new(KErrorKind::ShapeError(format!(
                        "LET name must be a bare type name, got `{}`",
                        name_kt.render(),
                    ))));
                }
                other => other.name(),
            };
            type_for_types_map = Some(match rhs {
                Held::Type(kt) => kt.clone(),
                // A module rides the Object arm but binds type-side: install its `KType::Module`
                // identity into `bindings.types` — today's storage shape — so `LET View = (m :| S)`
                // keeps a type-side binding and `-> Er.Carrier` elaboration resolves through it.
                Held::Object(KObject::Module(module)) => KType::Module { module },
                Held::Object(o) if matches!(o, KObject::KFunction(f) if f.is_functor) => o.ktype(),
                Held::Object(o) => {
                    return done_err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: o.ktype().name(),
                    }));
                }
            });
            resolved_name
        }
        (Some(other), None) => {
            return done_err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or ProperType".to_string(),
                got: other.ktype().name(),
            }));
        }
        (None, None) => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    if type_for_types_map.is_none() && ctx.scope.is_in_sig_body() {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write \
             `(VAL {name}: <Type>)` instead of `(LET {name} = <example-value>)`",
        ))));
    }
    if let Some(kt) = type_for_types_map {
        // Fused mint + alloc + register: the delivered RHS carrier's reach is minted into this scope's
        // arena (kept mode — a `KType` clone is shallow, so a module alias inherits the module's
        // child-scope reach and a region-pure / owned type reaches nothing), the identity is allocated
        // under it, and it is registered — one call returns the resident `&KType` plus the same token.
        let (kt_ref, stored) = match ctx.scope.register_user_type_delivered(
            name,
            kt,
            ctx.arg_carrier("value"),
            bind_index,
        ) {
            Ok(pair) => pair,
            Err(e) => return done_err(e),
        };
        // The terminal witnesses the aliased type in place from that same stored reach — the
        // reach-aware wrapper a later read uses.
        let carrier = ctx.scope.resident_type_carrier(kt_ref, stored);
        Action::Done(Ok(StepCarried::born(carrier)))
    } else {
        let value = rhs
            .as_object()
            .expect("value-route LET RHS is the Object arm");
        if matches!(value, KObject::KFunction(f) if f.is_functor) {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "a functor must be bound to a Type-class (capitalized) name; `{name}` \
                 is value-class — rebind under a Type-classified identifier instead \
                 (uppercase-leading plus at least one lowercase letter, e.g. `{suggestion}`)",
                suggestion = capitalize_identifier(&name),
            ))));
        }
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
        // Fused mint + copy + bind. A delivered RHS carrier derives the copy's stored reach in copied
        // mode — the deep-clone lands in this scope's own region, so a residence-only host is dropped
        // (`adopted_reach_of`, the same split the parameter and MATCH `it` binds apply) — and copies
        // the value in under it. A carrier-less region-pure RHS takes the checked tier, its
        // `(None, bit)` reach derived from the checked audit's own saw-a-region-pointer walk. Either
        // returns the resident reference plus the same token, from which the terminal witnesses the
        // bound value in place — the same reach-aware wrapper a later read uses.
        let bound = match ctx.arg_carrier("value") {
            Some(carrier) => ctx
                .scope
                .bind_delivered(name, carrier, bind_index, |carried| Ok(carried.object())),
            None => ctx.scope.bind_checked(name, value.deep_clone(), bind_index),
        };
        let (allocated, stored) = match bound {
            Ok(pair) => pair,
            Err(e) => return done_err(e),
        };
        Action::Done(Ok(StepCarried::born(
            ctx.scope.resident_value_carrier(allocated, stored),
        )))
    }
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

/// Dispatch-time placeholder extractor for the value-binding `LET <name> = …` overload:
/// matches only an `Identifier` name part. The type-alias overload (`LET <Type> = …`) uses
/// the shared [`type_part_binder_name`](crate::builtins::type_part_binder_name) instead, so
/// each overload's extractor matches exactly its own name-part kind. This keeps the
/// submit-time binder pick ([`extract_binder_install`]) selecting the correctly-classified
/// overload (the value extractor misses a `Type` part, and vice versa), so the placeholder is
/// tagged `Value` xor `Type` to match where the bind lands. Returns `None` on shape mismatch
/// (the body surfaces a structured error later).
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let identifier_sig = || {
        sig(
            KType::Any,
            vec![
                kw("LET"),
                arg("name", KType::Identifier),
                kw("="),
                arg("value", KType::Any),
            ],
        )
    };
    let type_sig = || {
        sig(
            KType::Any,
            vec![
                kw("LET"),
                arg("name", KType::OfKind(KKind::ProperType)),
                kw("="),
                arg("value", KType::Any),
            ],
        )
    };
    crate::builtins::register_builtin_full(
        scope,
        "LET",
        identifier_sig(),
        body,
        Some((binder_name, crate::machine::BindKind::Value)),
        None,
        false,
    );
    crate::builtins::register_builtin_full(
        scope,
        "LET",
        type_sig(),
        body,
        Some((super::type_part_binder_name, crate::machine::BindKind::Type)),
        None,
        false,
    );
}

#[cfg(test)]
mod tests;

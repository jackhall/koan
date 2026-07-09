use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::AbstractSource;
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
            if let Held::Type(kt) = rhs {
                let kind = match kt {
                    KType::Module { .. } => "module",
                    KType::Signature { .. } => "signature",
                    _ => "type",
                };
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
    let region = ctx.scope.brand();
    if let Some(kt) = type_for_types_map {
        let is_type_constructor = matches!(
            &kt,
            KType::SetRef { set, index }
                if set.member(*index).kind
                    == crate::machine::model::types::KKind::TypeConstructor
        );
        let kt = if ctx.scope.is_in_sig_body() && !is_type_constructor {
            KType::AbstractType {
                source: AbstractSource::Sig(ctx.scope.id),
                name: name.clone(),
            }
        } else {
            kt
        };
        // The aliased type's stored reach: minting the delivered RHS carrier's reach into this scope's
        // arena both computes the home-omitted foreign reach (a module alias inherits the module's
        // child-scope reach; a region-pure / owned type has none) AND pins it for the scope's life — no
        // separate fold, no walk of the built value.
        let stored = ctx
            .arg_carrier("value")
            .map(|carrier| ctx.scope.host_reach_of(carrier))
            .unwrap_or_default();
        if let Err(e) = ctx
            .scope
            .register_user_type(name, kt.clone(), bind_index, stored)
        {
            return done_err(e);
        }
        // The terminal witnesses the aliased type in place from that stored reach.
        let carrier = ctx.scope.resident_type_carrier(
            region.alloc_ktype(kt),
            stored.foreign,
            stored.borrows_into_home,
        );
        Action::Done(Ok(carrier))
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
        let allocated: &'a KObject<'a> = region.alloc_object(value.deep_clone());
        if allocated.is_unstamped_empty_container() {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "empty container bound to `{name}` has no element type to infer; \
                 annotate the value's type (e.g. via a typed FN return) or use a \
                 non-empty literal",
            ))));
        }
        // The bound value's stored reach. The RHS was `deep_clone`d into this scope's own region
        // above, so it does *not* reside in the delivered carrier's producer region: residence there
        // pins nothing, and the producer host materializes as a reach member only when the copy
        // genuinely borrows back into it (the copied-adoption rule — `adopted_reach_of`, the same
        // split the parameter and MATCH `it` binds already apply). Minting still both computes the
        // home-omitted foreign reach (stored on the binding so a later read rebuilds the value's
        // carrier from it) AND pins it for the scope's life — no separate fold. A region-pure RHS (no
        // delivered carrier) reaches nothing foreign, so the reach is empty.
        let stored = ctx
            .arg_carrier("value")
            .map(|carrier| ctx.scope.adopted_reach_of(carrier))
            .unwrap_or_default();
        if let Err(e) = ctx.scope.bind_value(name, allocated, bind_index, stored) {
            return done_err(e);
        }
        // The bound value lives in this scope's region with its stored foreign reach, so its terminal
        // carrier is built from that stored reach — the same reach-aware wrapper a later read uses —
        // rather than handed out as a bare `Done` for the finalize forward to wrap.
        Action::Done(Ok(ctx.scope.resident_value_carrier(
            allocated,
            stored.foreign,
            stored.borrows_into_home,
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

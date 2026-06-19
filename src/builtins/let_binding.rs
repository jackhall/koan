use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::AbstractSource;
use crate::machine::model::types::KKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `LET <name> = <value:Any>` — deep-clones the bound value into the arena and inserts it
/// under `name`. Two overloads share this body, differing only in the `name` slot's `KType`:
/// `Identifier` and `ProperType`. Same partition logic across both: reads its args from the
/// `BodyCtx::args` record, writes the binding directly on `ctx.scope` (interior-mutable), and
/// returns the bound carrier as `Action::Done`.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_held, arg_object, arg_type, Action};
    use crate::machine::model::values::Held;
    use crate::machine::model::Carried;

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
                Held::Object(o) if matches!(o, KObject::KFunction(f, _) if f.is_functor) => {
                    o.ktype()
                }
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
    let arena = ctx.scope.arena;
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
        let kt_ref: &'a KType<'a> = arena.alloc_ktype(kt.clone());
        if let Err(e) = ctx.scope.register_user_type(name, kt, bind_index) {
            return done_err(e);
        }
        Action::Done(Ok(Carried::Type(kt_ref)))
    } else {
        let value = rhs
            .as_object()
            .expect("value-route LET RHS is the Object arm");
        if matches!(value, KObject::KFunction(f, _) if f.is_functor) {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "a functor must be bound to a Type-class (capitalized) name; `{name}` \
                 is value-class — rebind under a Type-classified identifier instead \
                 (uppercase-leading plus at least one lowercase letter, e.g. `{suggestion}`)",
                suggestion = capitalize_identifier(&name),
            ))));
        }
        let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
        if allocated.is_unstamped_empty_container() {
            return done_err(KError::new(KErrorKind::ShapeError(format!(
                "empty container bound to `{name}` has no element type to infer; \
                 annotate the value's type (e.g. via a typed FN return) or use a \
                 non-empty literal",
            ))));
        }
        if let Err(e) = ctx.scope.bind_value(name, allocated, bind_index) {
            return done_err(e);
        }
        Action::Done(Ok(Carried::Object(allocated)))
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

/// Dispatch-time placeholder extractor for LET. Returns `None` on shape mismatch
/// (the body surfaces a structured error later).
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
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
                arg("name", KType::OfKind(KKind::Proper)),
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
        Some(binder_name),
        None,
        false,
    );
    crate::builtins::register_builtin_full(
        scope,
        "LET",
        type_sig(),
        body,
        Some(binder_name),
        None,
        false,
    );
}

#[cfg(test)]
mod tests;

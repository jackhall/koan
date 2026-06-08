use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::AbstractSource;
use crate::machine::model::types::KKind;
use crate::machine::model::{ArgValue, KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_with_binder, sig};

/// `LET <name> = <value:Any>` — deep-clones the bound value into the arena and
/// inserts it under `name`. Two overloads share this body, differing only in the
/// `name` slot's `KType`: `Identifier` and `TypeExprRef`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a, 'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Direct-body test fixtures bypass the scheduler and have no active chain;
    // [`BindingIndex::BUILTIN`] is always-visible so rebind/dedupe properties stay
    // testable in isolation.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    // The RHS rides either channel: a runtime value (including a functor `KFunction`) in
    // the Object arm, or a type-language identity (struct / union / module / signature /
    // Result / alias) raw in the Type arm.
    let rhs = match bundle.args.get("value") {
        Some(v) => v,
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    // `Some` routes the bind to `register_type` (type-side); `None` routes to
    // `bind_value` (value-side). A type-language alias (struct / union / module / Result /
    // signature) is always a single type-side write.
    let mut type_for_types_map: Option<KType<'a>> = None;
    let name = match (bundle.get("name"), bundle.get_type("name")) {
        // Identifier overload: a value-classified (lowercase-leading) binder name. The
        // partition invariant forbids it carrying any type-language value — a type aliases
        // only under a Type-classified name. See design/typing/elaboration.md § Binding-map
        // partition.
        (Some(KObject::KString(s)), _) => {
            if let ArgValue::Type(kt) = rhs {
                let kind = match kt {
                    KType::Module { .. } => "module",
                    KType::Signature { .. } => "signature",
                    _ => "type",
                };
                return err(KError::new(KErrorKind::ShapeError(format!(
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
        // `TypeExprRef` overload: the binder name rides the type channel — a bare leaf
        // (`Unresolved`) or a builtin-leaf identity. Structural type forms are not names.
        (_, Some(name_kt)) => {
            let resolved_name = match name_kt {
                KType::Unresolved(te) => te.render(),
                KType::List(_)
                | KType::Dict(_, _)
                | KType::KFunction { .. }
                | KType::KFunctor { .. }
                | KType::SetLocal(_)
                | KType::RecursiveRef(_) => {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "LET name must be a bare type name, got `{}`",
                        name_kt.render(),
                    ))));
                }
                other => other.name(),
            };
            // A Type-class binder name admits only a type-language RHS. A Type-arm identity
            // (struct / union / module / signature / Result / alias) registers directly: its
            // schema (or `&Module` / `&Signature`) rides the `KType`, so a plain `types`
            // write preserves dispatch identity. An Object-arm `is_functor` `KFunction`
            // registers its `KType::KFunctor { body: Some(f) }` projection so the callable
            // rides the type-table identity. A plain value rejects so `bindings.data` stays
            // the sole home for runtime values.
            type_for_types_map = Some(match rhs {
                ArgValue::Type(kt) => kt.clone(),
                ArgValue::Object(o) if matches!(&**o, KObject::KFunction(f, _) if f.is_functor) => {
                    o.ktype()
                }
                ArgValue::Object(o) => {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: o.ktype().name(),
                    }));
                }
            });
            resolved_name
        }
        (Some(other), None) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or TypeExprRef".to_string(),
                got: other.ktype().name(),
            }));
        }
        (None, None) => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    // Value slots inside a SIG body must use `(VAL <name>: <Type>)`. The check
    // fires only for the value-route so `LET Type = Number` and
    // `LET MyAlias = (some_module :| Sig)` keep working.
    if type_for_types_map.is_none() && scope.is_in_sig_body() {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write \
             `(VAL {name}: <Type>)` instead of `(LET {name} = <example-value>)`",
        ))));
    }
    let arena = scope.arena;
    if let Some(kt) = type_for_types_map {
        // Identity-preserving alias: `LET P2 = OrderedSig` writes `bindings.types[P2]`
        // carrying the aliased type's original identity so `(PICK x: P2)` and
        // `(PICK x: OrderedSig)` dispatch to the same overload. The alias binds at its
        // own lexical position, like every other binder.
        //
        // A SIG-local type binding (`LET Type = Number` inside a SIG body) binds the
        // name-bearing `AbstractType { source: Sig(decl_scope) }` rather than the collapsed
        // underlying type, so a later `VAL zero :Type` records that `zero` *names* the
        // abstract member. Opaque ascription threads this into the per-call module's
        // `slot_type_tags` and ATTR re-tags the slot read (see ascribe.rs / attr.rs). Only
        // a bare type LET inside a SIG is wrapped; outer aliases stay concrete.
        //
        // A higher-kinded `LET Wrap = (TEMPLATE T)` stays a `TypeConstructor`: ascription
        // already mints a fresh per-call constructor for those members (preserving the
        // higher-kinded shape), so collapsing it to an abstract scalar would lose the
        // parameterization.
        let is_type_constructor = matches!(
            &kt,
            KType::SetRef { set, index }
                if set.member(*index).kind
                    == crate::machine::model::types::KKind::TypeConstructor
        );
        let kt = if scope.is_in_sig_body() && !is_type_constructor {
            KType::AbstractType {
                source: AbstractSource::Sig(scope.id),
                name: name.clone(),
            }
        } else {
            kt
        };
        let kt_ref: &'a KType<'a> = arena.alloc_ktype(kt.clone());
        scope.register_type(name, kt, bind_index);
        BodyResult::ktype(kt_ref)
    } else {
        // The value route reaches here only for a value-classified binder name with an
        // Object-arm RHS (a type-language RHS errored above).
        let value = rhs
            .as_object()
            .expect("value-route LET RHS is the Object arm");
        // A functor lives in the type namespace only: a value-classified binder name
        // cannot host one (`register_type` is the sole legal home for the carried
        // `KType::KFunctor { body: Some(f) }`). Reject so `bindings.data` stays
        // unconditionally functor-free.
        if matches!(value, KObject::KFunction(f, _) if f.is_functor) {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "a functor must be bound to a Type-class (capitalized) name; `{name}` \
                 is value-class — rebind under a Type-classified identifier instead \
                 (uppercase-leading plus at least one lowercase letter, e.g. `{suggestion}`)",
                suggestion = capitalize_identifier(&name),
            ))));
        }
        let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
        // An untyped LET is a resolution boundary; an empty container with no
        // stamped element type would silently fix `List<Any>` / `Dict<Any, Any>`.
        // Force the user to annotate or use a non-empty literal.
        if allocated.is_unstamped_empty_container() {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "empty container bound to `{name}` has no element type to infer; \
                 annotate the value's type (e.g. via a typed FN return) or use a \
                 non-empty literal",
            ))));
        }
        if let Err(e) = scope.bind_value(name, allocated, bind_index) {
            return err(e);
        }
        BodyResult::value(allocated)
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
    register_builtin_with_binder(
        scope,
        "LET",
        sig(
            KType::Any,
            vec![
                kw("LET"),
                arg("name", KType::Identifier),
                kw("="),
                arg("value", KType::Any),
            ],
        ),
        body,
        Some(binder_name),
    );
    register_builtin_with_binder(
        scope,
        "LET",
        sig(
            KType::Any,
            vec![
                kw("LET"),
                arg("name", KType::OfKind(KKind::Proper)),
                kw("="),
                arg("value", KType::Any),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests;

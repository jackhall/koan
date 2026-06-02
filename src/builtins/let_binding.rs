use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::AbstractSource;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_with_binder, sig};

/// `LET <name> = <value:Any>` — deep-clones the bound value into the arena and
/// inserts it under `name`. Two overloads share this body, differing only in the
/// `name` slot's `KType`: `Identifier` and `TypeExprRef`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // Direct-body test fixtures bypass the scheduler and have no active chain;
    // [`BindingIndex::BUILTIN`] is always-visible so rebind/dedupe properties stay
    // testable in isolation.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let value = match bundle.require("value") {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // `Some` routes the bind to `register_type` (type-side); `None` routes to
    // `bind_value` (value-side). No nominal binder dual-writes anymore, so a
    // type-language alias (struct / union / module / Result / signature) is always a
    // single type-side write.
    let mut type_for_types_map: Option<KType<'a>> = None;
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => {
            // Partition guard: value-classified binder names cannot carry a module
            // or signature value. See design/typing/elaboration.md § Binding-map
            // partition.
            let kind = match value {
                KObject::KTypeValue(KType::Module { .. }) => Some("module"),
                KObject::KTypeValue(KType::Signature { .. }) => Some("signature"),
                _ => None,
            };
            if let Some(kind) = kind {
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
        Some(KObject::TypeNameRef(t)) => {
            let resolved_name = t.render();
            if !is_admissible_type_class_rhs(value) {
                return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                    name: resolved_name,
                    got: value.ktype().name(),
                }));
            }
            // Struct / union / module / Result / signature aliases are type-only:
            // their schema (or `&Module` / `&Signature`) rides the `KType` identity,
            // so a plain `types` write preserves dispatch identity without a
            // value-side copy.
            if let KObject::KTypeValue(kt) = value {
                type_for_types_map = Some(kt.clone());
            }
            resolved_name
        }
        // `TypeExprRef` overload: only leaf-named variants are valid binder names.
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::KFunctor { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            _ => {
                let resolved_name = t.name();
                if !is_admissible_type_class_rhs(value) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype().name(),
                    }));
                }
                // Struct / union / module / Result / signature aliases are type-only:
                // their schema (or `&Module` / `&Signature`) rides the `KType` identity,
                // so a plain `types` write preserves dispatch identity without a
                // value-side copy.
                if let KObject::KTypeValue(kt) = value {
                    type_for_types_map = Some(kt.clone());
                }
                resolved_name
            }
        },
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or TypeExprRef".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
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
    let cloned = value.deep_clone();
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc(cloned);
    if let Some(kt) = type_for_types_map {
        // Identity-preserving alias: `LET P2 = OrderedSig` writes `bindings.types[P2]`
        // carrying the aliased type's original identity so `(PICK x: P2)` and
        // `(PICK x: OrderedSig)` dispatch to the same overload. LET aliasing is
        // value-style gated — no `nominal_binder` carve-out; that's reserved for
        // STRUCT / SIG / FUNCTOR / MODULE / named UNION at their own install sites.
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
            kt,
            KType::UserType {
                kind: crate::machine::model::types::UserTypeKind::TypeConstructor { .. },
                ..
            }
        );
        let kt = if scope.is_in_sig_body() && !is_type_constructor {
            KType::AbstractType {
                source: AbstractSource::Sig(scope.id),
                name: name.clone(),
            }
        } else {
            kt
        };
        scope.register_type(name, kt, bind_index);
    } else {
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
    }
    BodyResult::Value(allocated)
}

/// Type-class LET allowlist. A Type-class binder name admits a value only if it
/// carries a type-language identity: any `KTypeValue(_)` (struct / union / module /
/// Result / signature identities all flow as `KTypeValue` now) or an `is_functor`-flagged
/// `KFunction`. Plain `KFunction` rejects so `LET Plain = (FN ...)` cannot silently land
/// under `bindings.data`. See
/// [design/typing/elaboration.md](../../design/typing/elaboration.md)
/// § Binding-map partition.
fn is_admissible_type_class_rhs<'a>(value: &KObject<'a>) -> bool {
    if matches!(value, KObject::KTypeValue(_)) {
        return true;
    }
    if let KObject::KFunction(f, _) = value {
        return f.is_functor;
    }
    false
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
                arg("name", KType::TypeExprRef),
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

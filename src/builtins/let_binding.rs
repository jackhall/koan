use crate::machine::model::{KObject, KType};
use crate::machine::model::types::UserTypeKind;
use crate::machine::{
    ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle,
};
use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::{arg, err, kw, register_builtin_with_pre_run, sig};

/// `LET <name> = <value:Any>` — copies the bound value into an arena-allocated `KObject`,
/// inserts it under `name`, and returns that same arena reference. Compound values recurse
/// through `KObject::deep_clone`.
///
/// Two overloads share this body, differing only in the `name` slot's `KType`: `Identifier`
/// (the original lowercase-name path) and `TypeExprRef` (so `LET ModuleName = (...)` can
/// bind a name that classifies as a Type token under the parser's token-classification rules).
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let value = match bundle.require("value") {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // `type_for_types_map` is `Some(kt)` iff this call should route storage through
    // `register_type` (a Type-class LHS with an actual `KTypeValue(kt)` RHS).
    // `nominal_identity` is `Some(kt)` iff the RHS is a type-language carrier with a
    // recoverable nominal identity (`KModule` / `KSignature` / `StructType` /
    // `TaggedUnionType`); those route through `register_nominal` so the alias name
    // resolves both type-side (via `resolve_type`) and value-side (via `lookup`).
    // Only one of the two is `Some` at any time — they're mutually exclusive RHS shapes.
    let mut type_for_types_map: Option<KType> = None;
    let mut nominal_identity: Option<KType> = None;
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        // Stage-2 carrier: a Type-classed binder name not in `KType::from_name`'s
        // builtin table lands as a `TypeNameRef`. Parameterized shapes (`List<X>`,
        // function arrow forms) are rejected — the binder name must be a bare leaf.
        // The `TypeClassBindingExpectsType` blocklist runs the same shape as the
        // `KTypeValue` arm: non-type RHS rejected before storage routing.
        Some(KObject::TypeNameRef(t)) => match &t.params {
            crate::machine::model::ast::TypeParams::List(_) | crate::machine::model::ast::TypeParams::Function { .. } => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            crate::machine::model::ast::TypeParams::None => {
                let resolved_name = t.name.clone();
                if matches!(
                    value.ktype(),
                    KType::Number | KType::Str | KType::Bool | KType::Null
                        | KType::List(_) | KType::Dict(_, _)
                ) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype(),
                    }));
                }
                if let KObject::KTypeValue(kt) = value {
                    type_for_types_map = Some(kt.clone());
                } else {
                    nominal_identity = derive_nominal_identity(value);
                }
                resolved_name
            }
        },
        // The `TypeExprRef` overload routes through `KTypeValue(kt)` post-refactor; only
        // leaf-named variants are valid binder names. Structural shapes (`List<X>`,
        // function types, `Mu` / `RecursiveRef`) are rejected as `ShapeError`.
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            _ => {
                // Bind-time rejection for `LET <Type-class> = <non-type>`. Blocklist —
                // not `value.ktype() != KType::TypeExprRef` — because the type-language
                // carriers `KModule` / `KSignature` / `StructType` / `TaggedUnionType`
                // report `Module` / `Signature` / `Type`, not `TypeExprRef`, and shipped
                // `LET IntOrdAbstract = (IntOrd :| OrderedSig)` patterns in `ascribe.rs`
                // depend on those being accepted. The non-`KTypeValue` carriers continue
                // to write `data` via `bind_value` until their own storage migration.
                let resolved_name = t.name();
                if matches!(
                    value.ktype(),
                    KType::Number | KType::Str | KType::Bool | KType::Null
                        | KType::List(_) | KType::Dict(_, _)
                ) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype(),
                    }));
                }
                // Stage-1.7 storage flip: Type-class LHS + `KTypeValue(kt)` RHS routes
                // through `register_type` so the bound name lives in `bindings.types`
                // alongside builtin type names. The dispatch carrier returned below
                // stays a `KObject::KTypeValue(kt)` — only the storage location moves.
                if let KObject::KTypeValue(kt) = value {
                    type_for_types_map = Some(kt.clone());
                } else {
                    nominal_identity = derive_nominal_identity(value);
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
    // SIG-body strict rejection: value slots inside a SIG body must use
    // `(VAL <name>: <Type>)`, not the ascription-by-example `(LET <name> = <value>)`
    // form. The check fires only for the value-route (neither Type-class LET nor a
    // nominal-identity carrier alias) so `LET Type = Number` and
    // `LET MyAlias = (some_module :| Sig)` keep working.
    if type_for_types_map.is_none() && nominal_identity.is_none() && scope.is_in_sig_body() {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "inside a SIG body, value slots must use VAL — write \
             `(VAL {name}: <Type>)` instead of `(LET {name} = <example-value>)`",
        ))));
    }
    let cloned = value.deep_clone();
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(cloned);
    if let Some(kt) = type_for_types_map {
        // Infallible `register_type` matches the prior `bind_value` shape for shipped
        // call sites (placeholder-resolution catches name conflicts upstream before
        // the body runs). The returned `KObject::KTypeValue(kt)` carrier is preserved
        // so dispatch transport — `lift_kobject`, the `value_lookup`-TypeExprRef
        // synthesis site, downstream `KType::TypeExprRef`-typed slots — sees the
        // same shape as before the storage flip.
        scope.register_type(name, kt);
    } else if let Some(identity) = nominal_identity {
        // Aliasing dual-write: `LET P2 = Point` writes `bindings.types[P2]` carrying
        // the ORIGINAL carrier's identity (Point's `name`/`scope_id`), not a fresh
        // identity minted from the alias name. This is what makes
        // `(PICK x: P2)` and `(PICK x: Point)` dispatch to the same overload — aliasing
        // preserves type identity rather than introducing a new nominal type.
        if let Err(e) = scope.register_nominal(name, identity, allocated) {
            return err(e);
        }
    } else {
        // Empty-container error rule: an untyped `LET` binding is an untyped resolution
        // boundary. An empty `[]` / `{}` with no stamped element type (carrier element
        // type `Any`) has no join to infer from and was never given a type by an
        // annotation upstream — binding it would silently fix `List<Any>` / `Dict<Any,
        // Any>`. Reject it; the user must annotate the producing boundary (an FN return
        // type) or use a non-empty literal.
        if allocated.is_unstamped_empty_container() {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "empty container bound to `{name}` has no element type to infer; \
                 annotate the value's type (e.g. via a typed FN return) or use a \
                 non-empty literal",
            ))));
        }
        if let Err(e) = scope.bind_value(name, allocated) {
            return err(e);
        }
    }
    BodyResult::Value(allocated)
}

/// Recover the nominal identity (a `KType::UserType` or `KType::SignatureBound`) carried
/// by a type-language value `obj`. Returns `Some(identity)` for the four shapes that came
/// from a STRUCT / UNION / MODULE / SIG declaration (or an alias of one); `None` for
/// every other carrier shape — those keep flowing through `Scope::bind_value` and never
/// dual-write to `bindings.types`.
///
/// The identity preserves the ORIGINAL declaration's `name` / `scope_id` rather than the
/// alias's binder name, so `LET P2 = Point` makes `P2` resolve to the same `UserType`
/// that `Point` carries.
fn derive_nominal_identity(obj: &KObject<'_>) -> Option<KType> {
    match obj {
        KObject::KModule(m, _) => Some(KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: m.scope_id(),
            name: m.path.clone(),
        }),
        KObject::KSignature(s) => Some(KType::SignatureBound {
            sig_id: s.sig_id(),
            sig_path: s.path.clone(),
            // A bare SIG alias (`LET S2 = OrderedSig`) carries no sharing constraints.
            pinned_slots: Vec::new(),
        }),
        KObject::StructType { name, scope_id, .. } => Some(KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: *scope_id,
            name: name.clone(),
        }),
        KObject::TaggedUnionType { name, scope_id, .. } => Some(KType::UserType {
            kind: UserTypeKind::Tagged,
            scope_id: *scope_id,
            name: name.clone(),
        }),
        _ => None,
    }
}

/// Dispatch-time placeholder extractor for LET. Both overloads (`LET <name:Identifier> = ...`
/// and `LET <name:TypeExprRef> = ...`) put the bound name at `parts[1]`; pull it out
/// structurally without dispatching anything. Returns `None` on shape mismatch (the body
/// will surface a structured error later).
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        ExpressionPart::Type(t) => Some(t.name.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "LET",
        sig(KType::Any, vec![
            kw("LET"),
            arg("name", KType::Identifier),
            kw("="),
            arg("value", KType::Any),
        ]),
        body,
        Some(pre_run),
    );
    register_builtin_with_pre_run(
        scope,
        "LET",
        sig(KType::Any, vec![
            kw("LET"),
            arg("name", KType::TypeExprRef),
            kw("="),
            arg("value", KType::Any),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;

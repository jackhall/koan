use crate::machine::model::{KObject, KType};
use crate::machine::model::types::UserTypeKind;
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, KError, KErrorKind, Scope, SchedulerHandle,
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
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // The LET body runs against the executing slot's lexical chain — `index` is the
    // statement position assigned at submission time. LET binders never carve out the
    // nominal-binder visibility flag (D7): a LET-bound value is strictly lexically gated.
    // Direct-body test fixtures that bypass the scheduler have no active chain; fall
    // back to [`BindingIndex::BUILTIN`] in that case — the visibility filter is "always
    // visible" so the lower-level rebind/dedupe properties stay testable in isolation.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);
    let value = match bundle.require("value") {
        Ok(v) => v,
        Err(e) => return err(e),
    };
    // `type_for_types_map` is `Some(kt)` iff this call should route storage through
    // `register_type` (a Type-class LHS with an actual `KTypeValue(kt)` RHS).
    // `nominal_identity` is `Some(kt)` iff the RHS is a type-language carrier with a
    // recoverable nominal identity (`KTypeValue(Module/Signature)` / `StructType` /
    // `TaggedUnionType`); those route through `register_nominal` so the alias name
    // resolves both type-side (via `resolve_type`) and value-side (via `lookup`).
    // Only one of the two is `Some` at any time — they're mutually exclusive RHS shapes.
    let mut type_for_types_map: Option<KType<'a>> = None;
    let mut nominal_identity: Option<KType<'a>> = None;
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => {
            // Partition guard: a value-classified binder name (lowercase-leading)
            // must not carry a module or signature value. Module/Signature carriers
            // belong on a Type-classified identifier (uppercase-leading + ≥1 lowercase
            // letter, per design/typing/tokens.md) so the type-side binding map is the
            // single home for module values — closes the asymmetry that lets a
            // value-side binding hide a category-mismatched module behind a lowercase
            // alias. See design/typing/elaboration.md § Binding home and the dual-map.
            let kind = match value {
                KObject::KTypeValue(KType::Module { .. }) => Some("module"),
                KObject::KTypeValue(KType::Signature(_)) => Some("signature"),
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
                // Type-class LET allowlist: a Type-class LET RHS must be a
                // type-language carrier (`KTypeValue`), a nominal-identity
                // carrier recoverable via `derive_nominal_identity` (Struct /
                // Tagged / Module / Signature alias), or an `is_functor`-flagged
                // KFunction (the FUNCTOR binder's output). Plain functions,
                // primitives, and containers all reject here rather than
                // silently landing under `bindings.data`. See
                // `is_admissible_type_class_rhs` below for the predicate.
                if !is_admissible_type_class_rhs(value) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype().name(),
                    }));
                }
                // Storage routing (unchanged): module / signature carriers dual-
                // write through `nominal_identity`; pure-type `KTypeValue(kt)`
                // carriers (Number, etc.) take the `register_type` path;
                // `is_functor` KFunctions and other nominal-identity carriers
                // (Struct / Tagged) fall through to the value-side binding.
                match value {
                    KObject::KTypeValue(KType::Module { .. } | KType::Signature(_)) => {
                        nominal_identity = derive_nominal_identity(value);
                    }
                    KObject::KTypeValue(kt) => {
                        type_for_types_map = Some(kt.clone());
                    }
                    _ => {
                        nominal_identity = derive_nominal_identity(value);
                    }
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
            | KType::KFunctor { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            _ => {
                // Same allowlist as the `TypeNameRef` arm above: type-language
                // carrier, nominal-identity carrier, or `is_functor`-flagged
                // KFunction. Plain `KFunction` (e.g. `LET Plain = (FN ...)`)
                // rejects rather than silently landing under `bindings.data`.
                let resolved_name = t.name();
                if !is_admissible_type_class_rhs(value) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype().name(),
                    }));
                }
                // Type-class LHS + value-routing. Module / signature carriers
                // (`KTypeValue(KType::Module/Signature)`) take the `nominal_identity`
                // path via `derive_nominal_identity` so they dual-write both
                // `bindings.types` (for type-position lookups) AND `bindings.data`
                // (for value-position lookups like `IntOrdView.compare`). Pure-type
                // `KTypeValue(kt)` carriers (Number, List<Any>, etc.) take the
                // `register_type` path — there's no useful value-side binding to
                // alias against. `is_functor` KFunctions and Struct / Tagged
                // carriers fall through to `bind_value`.
                match value {
                    KObject::KTypeValue(KType::Module { .. } | KType::Signature(_)) => {
                        nominal_identity = derive_nominal_identity(value);
                    }
                    KObject::KTypeValue(kt) => {
                        type_for_types_map = Some(kt.clone());
                    }
                    _ => {
                        nominal_identity = derive_nominal_identity(value);
                    }
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
    let allocated: &'a KObject<'a> = arena.alloc(cloned);
    if let Some(kt) = type_for_types_map {
        // Infallible `register_type` matches the prior `bind_value` shape for shipped
        // call sites (placeholder-resolution catches name conflicts upstream before
        // the body runs). The returned `KObject::KTypeValue(kt)` carrier is preserved
        // so dispatch transport — `lift_kobject`, the `value_lookup`-TypeExprRef
        // synthesis site, downstream `KType::TypeExprRef`-typed slots — sees the
        // same shape as before the storage flip.
        // Type-class LET RHS — value-side gated (no nominal-binder carve-out): the
        // alias is a let-style alias, not a fresh nominal type declaration.
        scope.register_type(name, kt, bind_index);
    } else if let Some(identity) = nominal_identity {
        // Aliasing dual-write: `LET P2 = Point` writes `bindings.types[P2]` carrying
        // the ORIGINAL carrier's identity (Point's `name`/`scope_id`), not a fresh
        // identity minted from the alias name. This is what makes
        // `(PICK x: P2)` and `(PICK x: Point)` dispatch to the same overload — aliasing
        // preserves type identity rather than introducing a new nominal type.
        //
        // LET aliasing is still value-style gating — `nominal_binder` stays `false`. A
        // proper nominal binder (STRUCT / SIG / FUNCTOR / MODULE / named UNION) sets it
        // at its own install site; an alias is the dual-map mirror of `LET x = expr`.
        if let Err(e) = scope.register_nominal(name, identity, allocated, bind_index) {
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
        if let Err(e) = scope.bind_value(name, allocated, bind_index) {
            return err(e);
        }
    }
    BodyResult::Value(allocated)
}

/// Recover the nominal identity carried by a type-language value `obj`. Returns
/// `Some(identity)` for the four shapes that came from a STRUCT / UNION / MODULE / SIG
/// declaration (or an alias of one); `None` for every other carrier shape — those keep
/// flowing through `Scope::bind_value` and never dual-write to `bindings.types`.
///
/// Post-collapse: MODULE/SIG carriers ride `KTypeValue(KType::Module/Signature)`; their
/// identity IS the carried KType, so the alias preserves the original `&Module` /
/// `&Signature` reference (which preserves `scope_id` / `sig_id` for downstream lookups,
/// the property `LET P2 = Point` already had for `Point`).
fn derive_nominal_identity<'a>(obj: &KObject<'a>) -> Option<KType<'a>> {
    match obj {
        // Module carrier: the slot-annotation form IS the carrier itself
        // (`KType::Module { module, frame }`). `(PICK m: AliasName)` should dispatch
        // identically to `(PICK m: OriginalName)`.
        KObject::KTypeValue(kt @ KType::Module { .. }) => Some(kt.clone()),
        // Signature carrier: slot-annotation form is the CONSTRAINT shape
        // `SatisfiesSignature { sig_id: s.sig_id(), .. }`. `(PICK m: S2)` and
        // `(PICK m: OrderedSig)` dispatch identically — both lower to the same
        // sig_id constraint. The value-side data binding still wraps the carrier
        // `KTypeValue(Signature(s))` so value-position uses see the signature value.
        KObject::KTypeValue(KType::Signature(s)) => Some(KType::SatisfiesSignature {
            sig_id: s.sig_id(),
            sig_path: s.path.clone(),
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

/// Type-class LET allowlist. A Type-class binder name (`LET MyName = <value>`
/// where `MyName` classifies as a Type token) admits a value only if it
/// carries a type-language identity in one of three shapes:
///
/// 1. `KObject::KTypeValue(_)` — pure-type carriers (`Number`, `:(List Str)`),
///    module / signature carriers (`KType::Module`, `KType::Signature`), and
///    abstract-type carriers all land here.
/// 2. A value whose `derive_nominal_identity` returns `Some(_)` —
///    `StructType` / `TaggedUnionType` carriers (and, redundantly with arm 1,
///    Module / Signature `KTypeValue`s).
/// 3. `KObject::KFunction(f, _)` where `f.is_functor` — the output of the
///    `FUNCTOR` binder. Plain `KFunction` (the FN output) rejects, so
///    `LET Plain = (FN …)` cannot silently bind a plain function under a
///    Type-class name.
///
/// Anything else surfaces `TypeClassBindingExpectsType`. See
/// [design/typing/elaboration.md](../../design/typing/elaboration.md)
/// (binding home and the dual-map) for the design rationale.
fn is_admissible_type_class_rhs<'a>(value: &KObject<'a>) -> bool {
    if matches!(value, KObject::KTypeValue(_)) {
        return true;
    }
    if derive_nominal_identity(value).is_some() {
        return true;
    }
    if let KObject::KFunction(f, _) = value {
        return f.is_functor;
    }
    false
}

/// Suggest a Type-classified rewrite of a value-classified binder name for the
/// Phase 1 partition-guard diagnostic. Capitalizes the first ASCII alphabetic
/// character so the result reads as a Type token (uppercase-leading plus at
/// least one lowercase, per design/typing/tokens.md). Falls back to a synthetic
/// `M` prefix if the name starts with a non-alphabetic character (digit / `_`)
/// where simple capitalization would not yield a Type-shape token.
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

//! Direct unit coverage for the `type_identity_for` helper. End-to-end coverage
//! of the dual-write itself lives in
//! [`crate::builtins::fn_def::tests::functor::dual_write`]; these tests pin
//! the per-row mapping in isolation without the surrounding scheduler.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::{RuntimeArena, ScopeId};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::values::{Module, Signature};

/// `SatisfiesSignature`-declared parameter bound to a `KModule` yields a
/// `UserType { kind: Module, scope_id, name }` identity.
#[test]
fn type_identity_for_signature_bound_yields_module_user_type() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Foo".into(),
    ));
    let module = arena.alloc_module(Module::new("Foo".into(), child));
    let obj = arena.alloc_object(KObject::KModule(module, None));
    let declared = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 42),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("module identity expected");
    assert_eq!(
        identity,
        KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: module.scope_id(),
            name: "Foo".into(),
        },
    );
}

/// `AnyUserType { kind: Module }`-declared parameter bound to a `KModule`
/// yields the same `UserType { kind: Module, .. }` identity. Mirrors the
/// `SatisfiesSignature` arm.
#[test]
fn type_identity_for_any_module_yields_module_user_type() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Bar".into(),
    ));
    let module = arena.alloc_module(Module::new("Bar".into(), child));
    let obj = arena.alloc_object(KObject::KModule(module, None));
    let declared = KType::AnyUserType { kind: UserTypeKind::Module };
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("module identity expected");
    assert_eq!(
        identity,
        KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: module.scope_id(),
            name: "Bar".into(),
        },
    );
}

/// `Signature`-declared parameter bound to a `KSignature` yields a bare
/// `SatisfiesSignature { sig_id, sig_path, pinned_slots: [] }` identity.
#[test]
fn type_identity_for_signature_yields_signature_bound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let obj = arena.alloc_object(KObject::KSignature(sig));
    let declared = KType::MetaSignature;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("signature identity expected");
    assert_eq!(
        identity,
        KType::SatisfiesSignature {
            sig_id: sig.sig_id(),
            sig_path: "OrderedSig".into(),
            pinned_slots: Vec::new(),
        },
    );
}

/// `Type`-declared parameter bound to a `KTypeValue(kt)` yields `kt.clone()`.
#[test]
fn type_identity_for_type_yields_inner_ktype() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::List(Box::new(KType::Number));
    let obj = arena.alloc_object(KObject::KTypeValue(inner.clone()));
    let declared = KType::Type;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

/// `TypeExprRef`-declared parameter bound to a `KTypeValue(kt)` yields
/// `kt.clone()` (the same arm as `Type`, since the carrier is the same).
#[test]
fn type_identity_for_type_expr_ref_kt_carrier_yields_inner_ktype() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::Number;
    let obj = arena.alloc_object(KObject::KTypeValue(inner.clone()));
    let declared = KType::TypeExprRef;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

/// Mismatched carrier for a type-denoting declared `KType` returns `Ok(None)` —
/// the dispatcher's `matches_value` filter already gated, so this path
/// indicates an `is_type_denoting` / `matches_value` disagreement (skip the
/// dual-write rather than panic).
#[test]
fn type_identity_for_carrier_mismatch_returns_none() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let obj = arena.alloc_object(KObject::Number(1.0));
    let declared = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    assert!(type_identity_for("p", obj, &declared, scope).expect("Ok expected").is_none());
}

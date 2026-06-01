//! Direct unit coverage for the `type_identity_for` helper. End-to-end coverage
//! of the per-call type-side bind itself lives in
//! [`crate::builtins::fn_def::tests::functor::per_call_type_side_bind`]; these
//! tests pin the per-row mapping in isolation without the surrounding scheduler.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::RuntimeArena;
use crate::machine::model::values::{Module, Signature};

#[test]
fn type_identity_for_signature_bound_yields_module_carrier() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Foo".into(),
    ));
    let module = arena.alloc_module(Module::new("Foo".into(), child));
    let obj = arena.alloc(KObject::KTypeValue(KType::Module {
        module,
        frame: None,
    }));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let declared = KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    };
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("module identity expected");
    assert_eq!(
        identity,
        KType::Module {
            module,
            frame: None
        }
    );
}

#[test]
fn type_identity_for_any_module_yields_module_carrier() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Bar".into(),
    ));
    let module = arena.alloc_module(Module::new("Bar".into(), child));
    let obj = arena.alloc(KObject::KTypeValue(KType::Module {
        module,
        frame: None,
    }));
    let declared = KType::AnyModule;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("module identity expected");
    assert_eq!(
        identity,
        KType::Module {
            module,
            frame: None
        }
    );
}

#[test]
fn type_identity_for_signature_yields_signature_carrier() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let obj = arena.alloc(KObject::KTypeValue(KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    }));
    let declared = KType::AnySignature;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("signature identity expected");
    assert_eq!(
        identity,
        KType::Signature {
            sig,
            pinned_slots: Vec::new()
        }
    );
}

#[test]
fn type_identity_for_type_yields_inner_ktype() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::List(Box::new(KType::Number));
    let obj = arena.alloc(KObject::KTypeValue(inner.clone()));
    let declared = KType::Type;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

#[test]
fn type_identity_for_type_expr_ref_kt_carrier_yields_inner_ktype() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::Number;
    let obj = arena.alloc(KObject::KTypeValue(inner.clone()));
    let declared = KType::TypeExprRef;
    let identity = type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

/// `matches_value` is supposed to have gated this case already; reaching the
/// helper with a mismatched carrier means `is_type_denoting` and `matches_value`
/// disagree, so skip the type-side install rather than panic.
#[test]
fn type_identity_for_carrier_mismatch_returns_none() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let obj = arena.alloc(KObject::Number(1.0));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let declared = KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    };
    assert!(type_identity_for("p", obj, &declared, scope)
        .expect("Ok expected")
        .is_none());
}

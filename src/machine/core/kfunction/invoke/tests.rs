//! Direct unit coverage for the `type_identity_for` helper. End-to-end coverage
//! of the per-call type-side bind itself lives in
//! [`crate::builtins::fn_def::tests::functor::per_call_type_side_bind`]; these
//! tests pin the per-row mapping in isolation without the surrounding scheduler.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::RuntimeArena;
use crate::machine::model::ast::TypeName;
use crate::machine::model::values::{Module, Signature};

#[test]
fn type_identity_for_module_yields_module_carrier() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Foo".into(),
    ));
    let module = arena.alloc_module(Module::new("Foo".into(), child));
    let kt = KType::Module {
        module,
        frame: None,
    };
    let identity = type_identity_for("p", &kt, scope)
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
    let kt = KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    };
    let identity = type_identity_for("p", &kt, scope)
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
fn type_identity_for_structural_type_yields_itself() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::List(Box::new(KType::Number));
    let identity = type_identity_for("p", &inner, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

#[test]
fn type_identity_for_leaf_type_yields_itself() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let inner = KType::Number;
    let identity = type_identity_for("p", &inner, scope)
        .expect("Ok expected")
        .expect("type identity expected");
    assert_eq!(identity, inner);
}

/// An `Unresolved` transient that doesn't resolve against the definition scope yields
/// `Ok(None)` — the type-side install is skipped and the body's value-side dispatch
/// surfaces the real error.
#[test]
fn type_identity_for_unbound_unresolved_name_returns_none() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let kt = KType::Unresolved(TypeName::leaf("Nonexistent".into()));
    assert!(type_identity_for("p", &kt, scope)
        .expect("Ok expected")
        .is_none());
}

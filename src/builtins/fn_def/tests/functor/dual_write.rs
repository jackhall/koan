//! Per-call type-side dual-write — functor bodies see the right `KType` for module-typed params at dispatch time.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

/// Held `KModule` from a functor must keep its child-scope arena alive across
/// subsequent run-root arena churn. End-to-end mirror of
/// [`crate::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`].
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET held_set = (MAKESET (int_ord_a))");

    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    run(scope, "LET other_set = (MAKESET (int_ord_a))");

    let data = scope.bindings().data();
    let m = match data.get("held_set") {
        Some(KObject::KModule(m, _)) => *m,
        other => panic!("held_set should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().bindings().data().get("inner").copied();
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
            "held_set.inner must still read 1.0 after subsequent churn");
}

/// Functor body resolves a type-class parameter via the per-call dual-write: without
/// it the body's auto-wrapped `(Er)` would hit `UnboundName` against the FN's
/// captured outer scope. Uses opaque ascription (`:|`) so the bound module carries
/// an abstract `Type` member for `MODULE_TYPE_OF` to return.
#[test]
fn functor_body_module_type_of_via_dual_write() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    run(
        scope,
        "FN (USE_TYPE Er :OrderedSig) -> Any = (MODULE_TYPE_OF Er Type)",
    );
    let result = run_one(scope, parse_one("USE_TYPE int_ord"));
    use crate::machine::model::KType;
    // Opaque ascription mints a fresh `UserType { kind: Module, name: "Type", .. }`
    // per ascription site (see `Module::type_members`); the body must return that
    // abstract identity, not the underlying concrete `Number`.
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::UserType { kind: crate::machine::model::types::UserTypeKind::Module, name, .. } => {
                assert_eq!(name, "Type", "abstract type member should be named Type");
            }
            other => panic!("expected UserType {{ kind :Module, name = \"Type\", .. }}, got {:?}", other),
        },
        other => panic!("expected KTypeValue carrying the abstract Type identity, got {:?}", other.ktype()),
    }
}

/// Type-side dual-write survives closure escape: an inner FN returned from an outer
/// functor reads its captured `Er` from the outer's per-call `bindings.types` even
/// after the outer call has returned. The `KFunction(&fn, Some(Rc<CallArena>))` lift
/// pins the value-side arena; this test pins the type-side entry alongside it.
#[test]
fn functor_closure_escape_pins_type_class_dual_write() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    run(
        scope,
        "FN (MAKE_LOOKUP Er :OrderedSig) -> Any = \
            (FN (LOOKUP) -> Any = (MODULE_TYPE_OF Er Type))",
    );
    run(scope, "LET _maker = (MAKE_LOOKUP int_ord)");
    // Churn exercises the per-call arena's drop discipline before the inner call.
    for _ in 0..5 {
        run_one(scope, parse_one("PRINT 1"));
    }
    let result = run_one(scope, parse_one("LOOKUP"));
    use crate::machine::model::KType;
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::UserType { kind: crate::machine::model::types::UserTypeKind::Module, name, .. } => {
                assert_eq!(name, "Type");
            }
            other => panic!(
                "expected UserType {{ kind: Module, name: \"Type\", .. }} \
                 after closure escape, got {:?}",
                other,
            ),
        },
        other => panic!(
            "expected KTypeValue carrying the abstract Type identity after closure escape, got {:?}",
            other.ktype(),
        ),
    }
}

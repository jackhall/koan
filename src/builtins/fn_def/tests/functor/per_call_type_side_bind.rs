//! Per-call type-side bind — functor bodies see the right `KType` for module-typed params at dispatch time.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// A held `KModule` from a functor body keeps its child-scope arena alive across
/// subsequent run-root churn. End-to-end mirror of
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
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET HeldSet = (MAKESET (IntOrdA))");

    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    run(scope, "LET OtherSet = (MAKESET (IntOrdA))");

    let m = match scope.resolve_type("HeldSet") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        other => panic!("HeldSet should be a module identity in types, got {other:?}"),
    };
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _)| *o);
    assert!(
        matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
        "HeldSet.inner must still read 1.0 after subsequent churn"
    );
}

/// Functor body resolves a type-class parameter via the per-call type-side bind:
/// without it the body's auto-wrapped `(Er)` would hit `UnboundName` against the
/// FN's captured outer scope. Uses opaque ascription (`:|`) so the bound module
/// carries an abstract `Type` member for the dotted `Er.Type` access to return.
#[test]
fn functor_body_dotted_type_member_via_per_call_bind() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(scope, "FN (USE_TYPE Er :OrderedSig) -> Any = (Er.Type)");
    let result = run_one(scope, parse_one("USE_TYPE IntOrdView"));
    use crate::machine::model::KType;
    // Opaque ascription mints a fresh abstract `Type` member; the body must return
    // that identity, not the underlying concrete `Number`.
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::AbstractType { name, .. } => {
                assert_eq!(name, "Type", "abstract type member should be named Type");
            }
            other => panic!(
                "expected AbstractType {{ name = \"Type\", .. }}, got {:?}",
                other
            ),
        },
        other => panic!(
            "expected KTypeValue carrying the abstract Type identity, got {:?}",
            other.ktype()
        ),
    }
}

/// Per-call type-side bind survives closure escape: an inner FN returned from an
/// outer functor reads its captured `Er` from the outer's per-call
/// `bindings.types` after the outer call has returned. The
/// `KFunction(&fn, Some(Rc<CallArena>))` lift pins the value-side arena; this
/// pins the type-side entry alongside it.
#[test]
fn functor_closure_escape_pins_type_class_bind() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(
        scope,
        "FN (MAKE_LOOKUP Er :OrderedSig) -> Any = \
            (FN (LOOKUP) -> Any = (Er.Type))",
    );
    run(scope, "LET _maker = (MAKE_LOOKUP IntOrdView)");
    // Churn the per-call arena's drop discipline before invoking the inner FN.
    for _ in 0..5 {
        run_one(scope, parse_one("PRINT 1"));
    }
    let result = run_one(scope, parse_one("LOOKUP"));
    use crate::machine::model::KType;
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::AbstractType { name, .. } => {
                assert_eq!(name, "Type");
            }
            other => panic!(
                "expected AbstractType {{ name: \"Type\", .. }} \
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

/// `FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)` dispatches and the
/// auto-wrapped `(Er)` body resolves `Er` through the per-call type-side binding,
/// returning the passed-through module without surfacing `UnboundName`.
#[test]
fn functor_returning_bare_signature_typed_param_does_not_panic() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET OrdView = (IntOrd :! OrderedSig)\n\
         FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)",
    );
    let result = run_one(scope, parse_one("MAKESET OrdView"));
    match result {
        KObject::KTypeValue(KType::Module { module, .. }) => {
            assert_eq!(module.path, "IntOrd :! OrderedSig");
        }
        other => panic!(
            "MAKESET OrdView must return the passed-through module carrier, got {:?}",
            other.ktype(),
        ),
    }
}

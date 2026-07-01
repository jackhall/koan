//! Per-call type-side bind — functor bodies see the right `KType` for module-typed params at dispatch time.

use crate::builtins::test_support::{parse_one, run, run_one, run_one_type, run_root_silent};
use crate::machine::core::FrameStorage;
use crate::machine::model::{KObject, KType};

/// A held `KModule` from a functor body keeps its child-scope region alive across
/// subsequent run-root churn. End-to-end mirror of
/// [`crate::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`].
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    run(scope, "LET HeldSet = (MAKESET (IntOrdA))");

    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    run(scope, "LET OtherSet = (MAKESET (IntOrdA))");

    let m = match scope.resolve_type("HeldSet") {
        Some(KType::Module { module: m }) => *m,
        other => panic!("HeldSet should be a module identity in types, got {other:?}"),
    };
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _, _)| *o);
    assert!(
        matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
        "HeldSet.inner must still read 1.0 after subsequent churn"
    );
}

/// Functor body resolves a type-class parameter via the per-call type-side bind:
/// without it the body's auto-wrapped `(Er)` would hit `UnboundName` against the
/// FN's captured outer scope. Uses opaque ascription (`:|`) so the bound module
/// carries an abstract `Carrier` member for the dotted `Er.Carrier` access to return.
#[test]
fn functor_body_dotted_type_member_via_per_call_bind() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(scope, "FN (USE_TYPE Er :OrderedSig) -> Any = (Er.Carrier)");
    let result = run_one_type(scope, parse_one("USE_TYPE IntOrdView"));
    // Opaque ascription mints a fresh abstract `Carrier` member; the body must return
    // that identity, not the underlying concrete `Number`.
    match result {
        KType::AbstractType { name, .. } => {
            assert_eq!(
                name, "Carrier",
                "abstract type member should be named Carrier"
            );
        }
        other => panic!("expected AbstractType {{ name = \"Carrier\", .. }}, got {other:?}"),
    }
}

/// Per-call type-side bind survives closure escape: an inner FN returned from an
/// outer functor reads its captured `Er` from the outer's per-call
/// `bindings.types` after the outer call has returned. The
/// `KFunction(&fn, Some(Rc<CallFrame>))` lift pins the value-side region; this
/// pins the type-side entry alongside it.
#[test]
fn functor_closure_escape_pins_type_class_bind() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = ((LET Carrier = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| OrderedSig)",
    );
    run(
        scope,
        "FN (MAKE_LOOKUP Er :OrderedSig) -> Any = \
            (FN (LOOKUP) -> Any = (Er.Carrier))",
    );
    run(scope, "LET _maker = (MAKE_LOOKUP IntOrdView)");
    // Churn the per-call region's drop discipline before invoking the inner FN.
    for _ in 0..5 {
        run_one(scope, parse_one("PRINT 1"));
    }
    let result = run_one_type(scope, parse_one("LOOKUP"));
    match result {
        KType::AbstractType { name, .. } => {
            assert_eq!(name, "Carrier");
        }
        other => panic!(
            "expected AbstractType {{ name: \"Carrier\", .. }} after closure escape, got {other:?}",
        ),
    }
}

/// `FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)` dispatches and the
/// auto-wrapped `(Er)` body resolves `Er` through the per-call type-side binding,
/// returning the passed-through module without surfacing `UnboundName`.
#[test]
fn functor_returning_bare_signature_typed_param_does_not_panic() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET OrdView = (IntOrd :! OrderedSig)\n\
         FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)",
    );
    let result = run_one_type(scope, parse_one("MAKESET OrdView"));
    match result {
        KType::Module { module, .. } => {
            assert_eq!(module.path, "IntOrd :! OrderedSig");
        }
        other => {
            panic!("MAKESET OrdView must return the passed-through module carrier, got {other:?}")
        }
    }
}

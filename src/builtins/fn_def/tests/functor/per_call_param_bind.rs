//! Per-call parameter bind — functor bodies see the right carrier for module-typed params at
//! dispatch time. A module argument binds value-side (`bindings.data`), so the body reads it back
//! as the Object-arm module value and projects members off it.

use crate::builtins::test_support::{
    lookup_module, parse_one, run, run_one, run_one_type, run_root_silent,
};
use crate::machine::core::run_root_storage;
use crate::machine::model::{KObject, KType, Parseable};

/// A held `KModule` from a functor body keeps its child-scope region alive across
/// subsequent run-root churn. End-to-end mirror of
/// [`crate::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`].
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    run(scope, "LET HeldSet = (MAKESET (IntOrdA))");

    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    run(scope, "LET OtherSet = (MAKESET (IntOrdA))");

    let m = lookup_module(scope, "HeldSet");
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

/// Functor body resolves a module-typed parameter via the per-call bind: without it the body's
/// auto-wrapped `(Er)` would hit `UnboundName` against the FN's captured outer scope. Uses opaque
/// ascription (`:|`) so the bound module carries an abstract `Carrier` member for the dotted
/// `Er.Carrier` access to return.
#[test]
fn functor_body_dotted_type_member_via_per_call_bind() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| Ordered)",
    );
    run(scope, "FN (USE_TYPE Er :Ordered) -> Any = (Er.Carrier)");
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

/// The per-call parameter bind survives closure escape: an inner FN returned from an outer functor
/// reads its captured `Er` from the outer's per-call `bindings.data` after the outer call has
/// returned. The `KFunction(&fn, Some(Rc<CallFrame>))` lift pins the region the binding lives in.
#[test]
fn functor_closure_escape_pins_type_class_bind() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         LET IntOrdView = (IntOrd :| Ordered)",
    );
    run(
        scope,
        "FN (MAKE_LOOKUP Er :Ordered) -> Any = \
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

/// `FN (MAKESET Er :Ordered) -> Ordered = (Er)` dispatches and the
/// auto-wrapped `(Er)` body resolves `Er` through the per-call value-side binding,
/// returning the passed-through module without surfacing `UnboundName`.
#[test]
fn functor_returning_bare_signature_typed_param_does_not_panic() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET OrdView = (IntOrd :! Ordered)\n\
         FN (MAKESET Er :Ordered) -> Ordered = (Er)",
    );
    let result = run_one(scope, parse_one("MAKESET OrdView"));
    match result {
        KObject::Module(module) => {
            assert_eq!(module.path, "IntOrd :! Ordered");
        }
        other => {
            panic!(
                "MAKESET OrdView must return the passed-through module carrier, got {}",
                other.summarize()
            )
        }
    }
}

//! Per-call parameter bind — functor bodies see the right carrier for module-typed params at
//! dispatch time. A module argument binds value-side (`bindings.data`), so the body reads it back
//! as the Object-arm module value and projects members off it.

use crate::builtins::test_support::{lookup_module, parse_one, TestRun};
use crate::machine::model::{KObject, TypeNode};
use crate::machine::run_root_storage;

/// A held `KModule` from a functor body keeps its child-scope region alive across
/// subsequent run-root churn. End-to-end mirror of
/// [`crate::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`].
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_ord_a = (int_ord :! Ordered)");
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))");
    test_run.run("LET held_set = (MAKESET (int_ord_a))");

    test_run.run("FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        test_run.run_one(parse_one("NOOP"));
    }
    test_run.run("LET other_set = (MAKESET (int_ord_a))");

    let m = lookup_module(scope, "held_set", &test_run.types);
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _, _)| *o);
    assert!(
        matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
        "held_set.inner must still read 1.0 after subsequent churn"
    );
}

/// Functor body resolves a module-typed parameter via the per-call bind: without it the body's
/// auto-wrapped `(er)` would hit `UnboundName` against the FN's captured outer scope. Uses opaque
/// ascription (`:|`) so the bound module carries an abstract `Carrier` member for the dotted
/// `er.Carrier` access to return.
#[test]
fn functor_body_dotted_type_member_via_per_call_bind() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         LET int_ord_view = (int_ord :| Ordered)",
    );
    test_run.run("FN (USE_TYPE er :Ordered) -> Any = (er.Carrier)");
    let result = test_run.run_one_type(parse_one("USE_TYPE int_ord_view"));
    // Opaque ascription mints a fresh abstract `Carrier` member; the body must return
    // that identity, not the underlying concrete `Number`.
    match test_run.types.node(result) {
        TypeNode::AbstractType { name, .. } => {
            assert_eq!(
                name, "Carrier",
                "abstract type member should be named Carrier"
            );
        }
        _ => panic!("expected AbstractType {{ name = \"Carrier\", .. }}, got {result:?}"),
    }
}

/// The per-call parameter bind survives closure escape: an inner FN returned from an outer functor
/// reads its captured `er` from the outer's per-call `bindings.data` after the outer call has
/// returned. The `KFunction(&fn, Some(Rc<CallFrame>))` lift pins the region the binding lives in.
#[test]
fn functor_closure_escape_pins_type_class_bind() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         LET int_ord_view = (int_ord :| Ordered)",
    );
    test_run.run(
        "FN (MAKE_LOOKUP er :Ordered) -> Any = \
            (FN (LOOKUP) -> Any = (er.Carrier))",
    );
    test_run.run("LET _maker = (MAKE_LOOKUP int_ord_view)");
    // Churn the per-call region's drop discipline before invoking the inner FN.
    for _ in 0..5 {
        test_run.run_one(parse_one("PRINT 1"));
    }
    let result = test_run.run_one_type(parse_one("LOOKUP"));
    match test_run.types.node(result) {
        TypeNode::AbstractType { name, .. } => {
            assert_eq!(name, "Carrier");
        }
        _ => panic!(
            "expected AbstractType {{ name: \"Carrier\", .. }} after closure escape, got {result:?}",
        ),
    }
}

/// `FN (MAKESET er :Ordered) -> Ordered = (er)` dispatches and the
/// auto-wrapped `(er)` body resolves `er` through the per-call value-side binding,
/// returning the passed-through module without surfacing `UnboundName`.
#[test]
fn functor_returning_bare_signature_typed_param_does_not_panic() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET ord_view = (int_ord :! Ordered)\n\
         FN (MAKESET er :Ordered) -> Ordered = (er)",
    );
    let result = test_run.run_one(parse_one("MAKESET ord_view"));
    match result {
        KObject::Module(module) => {
            // Ruling 12: the ascribed signature renders structurally, so the transparent-view
            // path label reads `int_ord :! SIG (compare: Number)`, not `:! Ordered`.
            assert_eq!(module.path, "int_ord :! SIG (compare: Number)");
        }
        other => {
            panic!(
                "MAKESET ord_view must return the passed-through module carrier, got {}",
                other.summarize(&test_run.types)
            )
        }
    }
}

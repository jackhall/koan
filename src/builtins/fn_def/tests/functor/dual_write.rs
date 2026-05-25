//! Per-call type-side dual-write — functor bodies see the right `KType` for module-typed params at dispatch time.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
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
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
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
            KType::AbstractType { name, .. } => {
                assert_eq!(name, "Type", "abstract type member should be named Type");
            }
            other => panic!("expected AbstractType {{ name = \"Type\", .. }}, got {:?}", other),
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

/// Roadmap regression test: `FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)`
/// dispatches and the auto-wrapped `(Er)` body resolves `Er` through the
/// per-call type-side binding (the data-side write was retired with the
/// type-language collapse — see [`crate::machine::core::kfunction::invoke`]'s
/// per-call binding loop). Pins that body resolution works end-to-end without
/// surfacing `UnboundName`.
///
/// **Caveat (residual `node_store.rs:169` panic at the interpret seam).** The
/// roadmap item promised this collapse would also dissolve the historical
/// `read_result` panic at [`crate::machine::execute::scheduler::node_store::NodeStore::read_result`]
/// — but only the *body-dispatch* shape did. The interpret seam in
/// [`crate::machine::execute::interpret::interpret_with_writer_path`] still
/// panics for top-level `SIG ...\nFN (... :SigName) -> ...` programs: the
/// FN-def's Combine (parked on `SigName`'s placeholder) calls `reclaim_deps`
/// on success, which eagerly frees the SIG dispatch's slot, after which
/// interpret's `read_result(top_level_id)` hits the freed slot and panics. The
/// fix is in the scheduler's eager-free policy, not the type-language layer
/// this item touches — see the residual-race note in the doc-shepherd
/// handoff. The unit-test path (`test_support::run` → `Scheduler::execute`)
/// does NOT call `read_result` per top-level id, so this regression remains
/// invisible to `cargo test` and the smoke must surface through the
/// `cargo run` integration path.
#[test]
fn functor_returning_bare_signature_typed_param_does_not_panic() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET ord = (IntOrd :! OrderedSig)\n\
         FN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)",
    );
    // Exercise the FUNCTOR: no panic, result is a module value matching the
    // OrderedSig constraint (the same `ord` module we passed in).
    let result = run_one(scope, parse_one("MAKESET ord"));
    match result {
        KObject::KTypeValue(KType::Module { module, .. }) => {
            assert_eq!(module.path, "IntOrd :! OrderedSig");
        }
        other => panic!(
            "MAKESET ord must return the passed-through module carrier, got {:?}",
            other.ktype(),
        ),
    }
}

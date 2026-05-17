//! Per-call type-side dual-write — functor bodies see the right `KType` for module-typed params at dispatch time.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

/// Module-system stage 2 (functor slice). End-to-end shape mirror of
/// [`crate::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`]:
/// run a complete functor invocation through the scheduler, hold the returned
/// `KModule` past several subsequent allocations and FN calls, and assert member
/// access on the lifted module still returns the correct value. Pins the closure-
/// escape + per-call-arena story for the functor result module under tree borrows.
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

    // Subsequent allocations and FN calls churn the run-root arena. The lifted
    // KModule must keep its child-scope arena alive through those churns.
    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    // Another functor call to allocate more frames (and drop them).
    run(scope, "LET other_set = (MAKESET (int_ord_a))");

    // Now read held_set's `inner` member — child_scope_ptr must still be live.
    let data = scope.bindings().data();
    let m = match data.get("held_set") {
        Some(KObject::KModule(m, _)) => *m,
        other => panic!("held_set should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().bindings().data().get("inner").copied();
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
            "held_set.inner must still read 1.0 after subsequent churn");
}

/// End-to-end per-call dual-write read-back: a FN `(USE_TYPE Er: OrderedSig)` whose
/// body does `(MODULE_TYPE_OF Er Type)` succeeds because `Er` resolves as a Type-class
/// reference through the per-call `bindings.types` entry the dual-write installs.
/// Without the dual-write, the body's auto-wrapped `(Er)` sub-Dispatch would hit
/// `value_lookup`'s TypeExprRef arm and surface `UnboundName(Er)` against the FN's
/// captured outer scope (where `Er` is per-call, not lexically present).
///
/// Uses opaque ascription (`:|`) so the bound module has an abstract `Type` member —
/// the body's `(MODULE_TYPE_OF Er Type)` lookup then has something to return, and
/// the value-side recovery path through `value_lookup::body_type_expr`'s dual-write
/// invariant (UserType { kind: Module, .. } → paired KModule carrier) hands the
/// module value to `MODULE_TYPE_OF` as expected.
///
/// Stage A only ships *parameter-position* dual-write — the FN return type stays on
/// the `Any` shape rather than templated forms like `-> (MODULE_TYPE_OF Er Type)`.
/// Stage B widens the return-type carrier to allow parameter-name references in
/// return-type positions.
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
    // Body invokes `(MODULE_TYPE_OF Er Type)`. The `Er` slot has declared type
    // `OrderedSig` (a `SignatureBound`); the dual-write puts `Er` into the per-call
    // `bindings.types` with the bound module's nominal identity, which the body's
    // auto-wrapped `(Er)` sub-Dispatch reads through `value_lookup`'s TypeExprRef
    // arm. The lookup returns the paired KModule carrier (per the dual-write
    // invariant in `value_lookup::body_type_expr`), feeding it into MODULE_TYPE_OF's
    // `m: Module` slot.
    run(
        scope,
        "FN (USE_TYPE Er :OrderedSig) -> Any = (MODULE_TYPE_OF Er Type)",
    );
    let result = run_one(scope, parse_one("USE_TYPE int_ord"));
    use crate::machine::model::KType;
    // The abstract `Type` member of an opaquely-ascribed IntOrd is a fresh
    // per-ascription `KType::UserType { kind: Module, name: "Type", .. }` (the
    // module-system pattern documented on `Module::type_members`). Verify the
    // body returned that abstract identity.
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

/// Closure-escape pin (Stage A A3). A FN that returns a nested FN closing over its
/// type-class parameter `Er` must keep the per-call scope alive long enough for the
/// nested FN's invocation to read `Er` from the per-call `bindings.types`. The
/// existing `KFunction(&fn, Some(Rc<CallArena>))` lift on the lifted FN value
/// already pins the per-call arena; this test confirms the type-side binding (not
/// just the value-side) survives the closure escape.
///
/// Shape: an outer functor whose body returns an inner FN whose body uses `Er` in a
/// type-position. Call the outer to get back the inner FN bound at the run-root
/// scope, then call the inner FN. If the per-call arena's scope (and the dual-write
/// it owns) outlives the inner FN value, the inner body's type-position resolution
/// of `Er` succeeds.
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
    // Outer FN captures `Er: OrderedSig`; its body defines an inner FN that returns
    // a `(MODULE_TYPE_OF Er Type)` value when called. The outer call binds `Er` on
    // the per-call scope (both value-side and type-side). The inner FN's captured
    // scope is the outer's per-call scope, so the inner body's reference to `Er`
    // walks up through the same scope.
    run(
        scope,
        "FN (MAKE_LOOKUP Er :OrderedSig) -> Any = \
            (FN (LOOKUP) -> Any = (MODULE_TYPE_OF Er Type))",
    );
    run(scope, "LET _maker = (MAKE_LOOKUP int_ord)");
    // Subsequent churn so the per-call arena's drop-discipline is exercised.
    for _ in 0..5 {
        run_one(scope, parse_one("PRINT 1"));
    }
    // Now invoke the nested LOOKUP. The inner FN's captured scope still holds the
    // per-call `Er -> UserType { kind: Module, .. }` entry installed by the outer
    // call's dual-write.
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

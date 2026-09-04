//! The window over an **opaque view**: every member read by bare name inside the block reports the
//! same view-side type the ATTR read reports.
//!
//! The window borrows the view scope's binding table wholesale
//! ([`Scope::open_module_window`](crate::machine::Scope)), so this needs no machinery of its own —
//! the view's scope is born holding coerced member values, and the block reads that same table.
//! These pin that equivalence for a first-order slot, an applied-constructor slot, and a
//! function-typed slot called inside the block — and, on the keyworded channel, that the window
//! surfaces only the dispatch buckets the view's signature declares.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

/// A first-order abstract slot plus an applied one, a function-typed slot, and the source
/// constructors both are declared over — the same shape `ascribe/tests/views.rs` reads through
/// ATTR, read here by bare name instead.
fn view_program() -> &'static str {
    "NEWTYPE (Type AS Wrapper)\n\
     NEWTYPE Carrier = Number\n\
     SIG Monad = ((TYPE (Type AS Wrap)) (TYPE Elt) \
     (VAL boxed :(Number AS Wrap)) (VAL zero :Elt) \
     (VAL pure :(FN :{x :Number} -> :(Number AS Wrap))))\n\
     MODULE id_monad = ((LET Wrap = Wrapper) (LET Elt = Carrier) \
     (LET boxed = (Wrapper (3))) (LET zero = (Carrier 0)) \
     (LET pure = FN (PURE x :Number) -> :(Number AS Wrapper) = (Wrapper (x))))\n\
     LET view = (id_monad :| Monad)\n\
     FN (TAKEVIEW x :(Number AS view.Wrap)) -> Number = (2)\n\
     FN (TAKESRC x :(Number AS Wrapper)) -> Number = (1)\n\
     FN (TAKEELT x :(view.Elt)) -> Number = (3)\n\
     FN (TAKECARRIER x :Carrier) -> Number = (4)"
}

/// A **first-order** abstract slot read inside the window carries the view's own mint: it
/// satisfies the view's `Elt` and is a dispatch miss against the source's `Carrier`.
#[test]
fn window_read_of_a_first_order_slot_is_the_views_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(view_program());

    let result = test_run.run_one(test_run.parse_one("USING view SCOPE (TAKEELT (zero))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "the window read must satisfy the view's own `Elt`, got {}",
        result.summary(test_run.registries()),
    );
    let err = test_run.run_one_err(test_run.parse_one("USING view SCOPE (TAKECARRIER (zero))"));
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "and must not satisfy the source's `Carrier`, got {err}",
    );
}

/// An **applied-constructor** slot behaves the same inside the window as through ATTR.
#[test]
fn window_read_of_an_applied_slot_is_the_views_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(view_program());

    let result = test_run.run_one(test_run.parse_one("USING view SCOPE (TAKEVIEW (boxed))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 2.0),
        "the window read must satisfy `:(Number AS view.Wrap)`, got {}",
        result.summary(test_run.registries()),
    );
    let err = test_run.run_one_err(test_run.parse_one("USING view SCOPE (TAKESRC (boxed))"));
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "and must not satisfy the source's applied type, got {err}",
    );
}

/// A **function-typed** slot called by bare name inside the window returns the view's types: the
/// member the window surfaces is the coercion wrapper, not the underlying callable.
#[test]
fn window_call_of_a_function_slot_returns_the_views_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(view_program());

    let result = test_run.run_one(test_run.parse_one("USING view SCOPE (TAKEVIEW (pure {x = 2}))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 2.0),
        "the call's result must satisfy the view's applied type, got {}",
        result.summary(test_run.registries()),
    );
    let err = test_run.run_one_err(test_run.parse_one("USING view SCOPE (TAKESRC (pure {x = 2}))"));
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "and must not satisfy the source's, got {err}",
    );
}

/// A **transparent** view's window stays concrete: nothing was coerced, so a bare-name read inside
/// the block satisfies the source's types exactly as a direct read does.
#[test]
fn window_over_a_transparent_view_stays_concrete() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(view_program());
    test_run.run("LET tview = (id_monad :! Monad)");

    let result = test_run.run_one(test_run.parse_one("USING tview SCOPE (TAKESRC (boxed))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 1.0),
        "a transparent window's read stays at the source's applied type, got {}",
        result.summary(test_run.registries()),
    );
}

/// The **keyworded channel** the window borrows is the view's own, and a view surfaces only the
/// buckets its signature declares. So a call whose key the signature omits is not shadowed by the
/// module inside the window — it walks on out to the enclosing scope's overload, while the same
/// window over the *source* module resolves the module's.
#[test]
fn a_window_over_a_view_surfaces_only_the_declared_buckets() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Tagged = ((VAL tag :Number))\n\
         MODULE tagged_m = ((LET tag = 1) (FN (HIDE x :Number) -> Number = (1)))\n\
         LET view = (tagged_m :| Tagged)\n\
         FN (HIDE x :Number) -> Number = (99)",
    );

    let outer = test_run.run_one(test_run.parse_one("USING view SCOPE (HIDE 5)"));
    assert!(
        matches!(outer, KObject::Number(n) if *n == 99.0),
        "an undeclared bucket is absent from the view, so the window shadows nothing, got {}",
        outer.summary(test_run.registries()),
    );
    let inner = test_run.run_one(test_run.parse_one("USING tagged_m SCOPE (HIDE 5)"));
    assert!(
        matches!(inner, KObject::Number(n) if *n == 1.0),
        "the source module's own window still shadows the enclosing overload, got {}",
        inner.summary(test_run.registries()),
    );
}

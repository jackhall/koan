//! After the outer submission of `LET f = (FN (HELPER x :Number) -> Number =
//! (x))` returns and before any node runs, BOTH the LET's name placeholder
//! AND the inner FN's pending-overload bucket `[HELPER, Slot]` must be
//! installed in the dispatching scope's `bindings`. Otherwise a sibling that
//! dispatches a call shape matching the still-uninstalled bucket would
//! hard-error under strict-only admission instead of parking.

use crate::builtins::test_support::TestRun;
use crate::machine::core::run_root_storage;
use crate::machine::model::UntypedElement;
use crate::parse::parse;

#[test]
fn nested_binder_installs_inner_placeholder_at_outer_submission() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let mut exprs =
        parse("LET f = (FN (HELPER x :Number) -> Number = (x))").expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test fixture: single top-level expression");
    let expr = exprs.remove(0);
    let _id = test_run.runtime.dispatch_in_scope(expr, scope);
    // Read both maps before any `execute()` — installs must land at submission time.
    let placeholders = scope.bindings().placeholders();
    assert!(
        placeholders.contains_key("f"),
        "outer LET should install placeholder `f` at submission; \
         placeholders = {:?}",
        placeholders.keys().collect::<Vec<_>>(),
    );
    drop(placeholders);
    let pending = scope.bindings().pending_overloads();
    let helper_bucket = vec![
        UntypedElement::Keyword("HELPER".to_string()),
        UntypedElement::Slot,
    ];
    assert!(
        pending.contains_key(&helper_bucket),
        "inner FN (pre-submitted as a sub-Dispatch of LET) should install \
         pending-overload bucket [HELPER, Slot] at submission; \
         pending_overloads = {:?}",
        pending.keys().collect::<Vec<_>>(),
    );
}

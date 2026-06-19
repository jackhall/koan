//! After the outer submission of `LET f = (FN (HELPER x :Number) -> Number =
//! (x))` returns and before any node runs, BOTH the LET's name placeholder
//! AND the inner FN's pending-overload bucket `[HELPER, Slot]` must be
//! installed in the dispatching scope's `bindings`. Otherwise a sibling that
//! dispatches a call shape matching the still-uninstalled bucket would
//! hard-error under strict-only admission instead of parking.

use std::io::Write;

use crate::builtins::default_scope;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::types::UntypedElement;
use crate::machine::KoanRegion;
use crate::parse::parse;

struct Sink;
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn nested_binder_installs_inner_placeholder_at_outer_submission() {
    let arena = KoanRegion::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let mut exprs =
        parse("LET f = (FN (HELPER x :Number) -> Number = (x))").expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test fixture: single top-level expression");
    let expr = exprs.remove(0);
    let mut sched = KoanRuntime::new();
    let _id = sched.dispatch_in_scope(expr, scope);
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

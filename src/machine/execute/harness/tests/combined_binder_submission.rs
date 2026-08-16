//! The combined statement form installs both channels *atomically*, at submission: after
//! `LET f = FN (HELPER x :Number) -> Number = (x)` is submitted and before any node runs, BOTH the
//! name claim on `f` AND the pending slot in bucket `[HELPER, Slot]` must be in the dispatching
//! scope's `bindings`. Otherwise a sibling dispatching a call shape matching the still-uninstalled
//! bucket would hard-error under strict-only admission instead of parking.

use super::working_one;
use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::UntypedElement;

#[test]
fn combined_form_installs_both_channels_at_submission() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let expr = working_one(&program, "LET f = FN (HELPER x :Number) -> Number = (x)");
    let _id = test_run.dispatch_in_scope(expr, scope);
    // Read both tables before any `execute()` — installs must land at submission time.
    assert!(
        scope.bindings().pending_value("f").is_some(),
        "the combined statement should claim `f`'s value slot at submission; \
         pending = {:?}",
        scope.bindings().pending_names(),
    );
    let helper_bucket = vec![
        UntypedElement::Keyword("HELPER".to_string()),
        UntypedElement::Slot,
    ];
    assert!(
        !scope
            .bindings()
            .pending_overload_entries(&helper_bucket)
            .is_empty(),
        "the combined statement's own binder plan should claim a pending slot in \
         bucket [HELPER, Slot] at submission",
    );
}

//! The combined statement form installs both channels *atomically*, at submission: after
//! `LET f = FN (HELPER x :Number) -> Number = (x)` is submitted and before any node runs, BOTH the
//! name claim on `f` AND the bucket claim on `[HELPER, Slot]` must be in the dispatching
//! scope's `bindings`. Otherwise a sibling dispatching a call shape matching the still-uninstalled
//! bucket would hard-error under strict-only admission instead of parking.

use super::working_one;
use crate::builtins::test_support::key_keyword;
use crate::builtins::test_support::{TestRun, value_name};
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::KeyElement;

#[test]
fn combined_form_installs_both_channels_at_submission() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let expr = working_one(
        &program,
        &test_run.registries().labels,
        "LET f = FN (HELPER x :Number) -> Number = (x)",
    );
    let _id = test_run.dispatch_in_scope(expr, scope);
    // Read both tables before any `execute()` — installs must land at submission time.
    assert!(
        scope
            .bindings()
            .pending_value(value_name("f", test_run.registries()))
            .is_some(),
        "the combined statement should claim `f`'s value slot at submission; \
         pending = {:?}",
        scope.bindings().pending_names(test_run.registries()),
    );
    let helper_bucket = vec![key_keyword("HELPER"), KeyElement::Slot];
    assert!(
        !scope
            .bindings()
            .pending_overload_entries(&helper_bucket)
            .is_empty(),
        "the combined statement's own binder plan should claim the bucket \
         bucket [HELPER, Slot] at submission",
    );
}

//! Miss-diagnosis tests: the write door's refusal of a user claim on a reserved key. That a
//! diagnosing key names a live bucket and a reserved one names none is the form table's own law,
//! checked against the single live-registration walk in `parse::forms::tests::registration`.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::memory::{program_storage, run_root_storage};

/// A user `FN` whose signature spells a reserved key is refused at the write door, exactly as a
/// builtin-bucket shadow is. Without the refusal the shape would resolve to a user body, and a
/// genuine typed miss under that bucket would render the reserved shape's targeted message.
#[test]
fn a_user_registration_under_a_reserved_key_is_refused() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(
        test_run
            .parse_one("EXPR (UNARY OP sym :Str OVER operand :Str = body :Str) -> Number = (1)"),
    );
    assert!(
        matches!(&error.kind, KErrorKind::Rebind { name } if name.contains("UNARY")),
        "expected the reserved-key refusal, got {error}",
    );
}

//! Miss-diagnosis tests: the table⟺registration consistency pin (a diagnosing key names a live
//! bucket, a reserved one names none) and the write door's refusal of a user claim on a reserved
//! key.

use std::collections::HashSet;

use super::MISS_DIAGNOSTICS;
use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::model::UntypedKey;
use crate::machine::model::key_spec::{FORMS, Form, FormId, key_matches, render_key};
use crate::memory::{program_storage, run_root_storage};

/// Every bucket key the seeded root registers a callable under.
fn live_buckets() -> HashSet<UntypedKey> {
    let program = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    run.scope
        .ancestors()
        .flat_map(|scope| {
            scope
                .bindings()
                .functions()
                .iter()
                .map(|(key, _)| key.to_vec())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The [`FORMS`] entry a diagnosis tags.
fn form_of(id: FormId) -> &'static Form {
    FORMS
        .iter()
        .find(|form| form.id == id)
        .expect("every tag names a table entry")
}

/// A diagnosis on a non-reserved form rides a key that *does* register: its render fires only on
/// the shape its success-path siblings reject, so a key whose builtin was renamed, re-shaped, or
/// dropped would leave the entry diagnosing a bucket nothing reaches.
#[test]
fn every_diagnosing_entry_names_a_live_bucket() {
    let live = live_buckets();
    for (id, _) in MISS_DIAGNOSTICS {
        let form = form_of(*id);
        if form.reserved {
            continue;
        }
        assert!(
            live.iter()
                .any(|key| key_matches(form.key, key.iter().copied())),
            "miss-diagnosis key {:?} has no registered bucket",
            render_key(form.key)
        );
    }
}

/// The other kind: a reserved form's whole point is that nothing registers under its key. A
/// registration appearing there would mean the shape has a success reading after all, and its
/// diagnosis would be describing a form that works.
#[test]
fn every_reserved_form_names_no_bucket() {
    let live = live_buckets();
    for form in FORMS.iter().filter(|form| form.reserved) {
        assert!(
            !live
                .iter()
                .any(|key| key_matches(form.key, key.iter().copied())),
            "reserved form key {:?} has a registered bucket",
            render_key(form.key)
        );
    }
}

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

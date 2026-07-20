//! Structural record types in FN parameter and return slots: `:{x :Number, y :Str}`,
//! width/depth subtyping, and specificity tournaments. Records subtype the *dual* way to
//! function params — a wider record value is more specific (fills a narrower slot).

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

use super::capture_program_output;

/// A `{x = 1, y = "a"}` value fills a `:{x :Number, y :Str}` parameter slot and the
/// function runs.
#[test]
fn fn_with_record_param_accepts_matching_record() {
    let bytes = capture_program_output(
        "FN (USE r :{x :Number, y :Str}) -> Str = (\"ok\")\n\
         PRINT (USE {x = 1, y = \"a\"})",
    );
    assert_eq!(bytes, b"ok\n");
}

/// Width-drop admit: a wider `{x = 1, y = "a"}` value fills a narrower `:{x :Number}`
/// slot — the extra field `y` is dropped through the narrowed type.
#[test]
fn fn_with_record_param_admits_wider_value() {
    let bytes = capture_program_output(
        "FN (USE r :{x :Number}) -> Str = (\"ok\")\n\
         PRINT (USE {x = 1, y = \"a\"})",
    );
    assert_eq!(bytes, b"ok\n");
}

/// Width specificity: a `{x = 1, y = "a"}` call picks the wider `:{x :Number, y :Str}`
/// overload over the narrower `:{x :Number}` — a superset record is strictly more
/// specific.
#[test]
fn dispatch_picks_wider_record_overload() {
    let bytes = capture_program_output(
        "FN (USE r :{x :Number}) -> Str = (\"narrow\")\n\
         FN (USE r :{x :Number, y :Str}) -> Str = (\"wide\")\n\
         PRINT (USE {x = 1, y = \"a\"})",
    );
    assert_eq!(bytes, b"wide\n");
}

/// Depth specificity (covariant): a `{x = 1}` call picks `:{x :Number}` over `:{x :Any}`.
#[test]
fn dispatch_picks_deeper_record_overload() {
    let bytes = capture_program_output(
        "FN (PICK r :{x :Any}) -> Str = (\"any\")\n\
         FN (PICK r :{x :Number}) -> Str = (\"num\")\n\
         PRINT (PICK {x = 1})",
    );
    assert_eq!(bytes, b"num\n");
}

/// A record-typed return slot round-trips: the body's `{x = 1, y = "a"}` satisfies the
/// declared `:{x :Number, y :Str}` and renders back in record surface form.
#[test]
fn fn_returning_record_accepts_matching_value() {
    let bytes = capture_program_output(
        "FN (MK) -> :{x :Number, y :Str} = ({x = 1, y = \"a\"})\n\
         PRINT (MK)",
    );
    assert_eq!(bytes, b"{x = 1, y = a}\n");
}

/// A record value (`ktype()` carries the field-type record) reports a `KType::Record`.
#[test]
fn record_value_reports_record_ktype() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("{x = 1, y = \"a\"}"));
    assert_eq!(
        result.ktype(),
        KType::record(Box::new(Record::from_pairs(vec![
            ("x".into(), KType::Number),
            ("y".into(), KType::Str),
        ]))),
    );
}

/// Field-type mismatch is a dispatch non-match: an evaluated `{x = "s"}` (carried
/// `:{x :Str}`) does not satisfy a `:{x :Number}` slot, and with no other overload the
/// call surfaces `DispatchFailed` (a bound variable exercises the carried-type gate
/// rather than the shape-only literal path).
#[test]
fn record_field_type_mismatch_is_dispatch_failure() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("LET r = {x = \"s\"}");
    test_run.run("FN (USE r :{x :Number}) -> Str = (\"ok\")");
    let root = test_run
        .runtime
        .dispatch_in_scope(parse_one("USE r"), scope);
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = test_run
        .runtime
        .result_error(root)
        .expect_err("a `:{x :Str}` value must not fill a `:{x :Number}` slot");
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed on record field-type mismatch, got {error:?}",
    );
}

/// Missing-field reject: an evaluated `{x = 1}` does not fill a slot demanding a field it
/// lacks (`:{x :Number, q :Bool}`) — the value can't satisfy the wider promise.
#[test]
fn record_missing_field_is_dispatch_failure() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("LET r = {x = 1}");
    test_run.run("FN (NEED r :{x :Number, q :Bool}) -> Str = (\"ok\")");
    let root = test_run
        .runtime
        .dispatch_in_scope(parse_one("NEED r"), scope);
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = test_run
        .runtime
        .result_error(root)
        .expect_err("a `{x = 1}` value must not fill a `:{x :Number, q :Bool}` slot");
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed on missing record field, got {error:?}",
    );
}

/// Incomparable arms tie as ambiguous: a `{x = 1, y = "a", z = "b"}` value is a superset
/// of both `:{x :Number, y :Str}` and `:{x :Number, z :Str}`, so it fills both; the two
/// slots are mutually incomparable (disjoint extra fields), so neither wins →
/// `AmbiguousDispatch`.
#[test]
fn record_incomparable_overloads_are_ambiguous() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (PICK r :{x :Number, y :Str}) -> Str = (\"xy\")");
    test_run.run("FN (PICK r :{x :Number, z :Str}) -> Str = (\"xz\")");
    let root = test_run
        .runtime
        .dispatch_in_scope(parse_one("PICK {x = 1, y = \"a\", z = \"b\"}"), scope);
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = test_run
        .runtime
        .result_error(root)
        .expect_err("a value matching two incomparable record slots must be ambiguous");
    assert!(
        matches!(error.kind, KErrorKind::AmbiguousDispatch { .. }),
        "expected AmbiguousDispatch across incomparable record slots, got {error:?}",
    );
}

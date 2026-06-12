//! TRY-WITH branch dispatch over success and per-`KErrorKind` arms, plus re-raise on
//! no-match and wildcard `_` coverage of dispatcher-internal kinds.

use crate::builtins::test_support::{
    parse_one, run, run_one_err, run_root_silent, run_root_with_buf,
};
use crate::machine::{KErrorKind, RuntimeArena};

fn run_program(source: &str) -> Vec<u8> {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}

#[test]
fn ok_arm_runs_on_success_and_binds_it_to_value() {
    let bytes = run_program("TRY (PRINT \"hello\") -> :Str WITH (Ok -> (PRINT \"caught ok\"))");
    assert_eq!(bytes, b"hello\ncaught ok\n");
}

#[test]
fn ok_binds_it_to_success_value() {
    let bytes = run_program("TRY (PRINT \"value\") -> :Str WITH (Ok -> (PRINT it))");
    assert_eq!(bytes, b"value\nvalue\n");
}

#[test]
fn arm_violating_declared_return_type_errors() {
    // Declared `:Number`, but the `ok` arm returns a Str (PRINT's rendered string).
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("TRY (PRINT \"v\") -> :Number WITH (Ok -> (PRINT \"caught\"))"),
    );
    assert!(
        matches!(&err.kind, KErrorKind::TypeMismatch { arg, .. } if arg == "<return>"),
        "expected <return> TypeMismatch from the arm result, got {err}",
    );
}

#[test]
fn unbound_name_arm_catches_unbound_name() {
    let bytes = run_program(
        "TRY (foo) -> :Str WITH (\
            Ok -> (PRINT \"ok\")\
            UnboundName -> (PRINT it.name)\
         )",
    );
    assert_eq!(bytes, b"foo\n");
}

#[test]
fn dispatch_failed_arm_catches_keyworded_dispatch_failure() {
    // A Keyworded overload miss (type-mismatched arg, no matching FN) is slot-terminal,
    // so TRY intercepts it rather than the run aborting at the `execute()` boundary.
    let bytes = run_program(
        "FN (DOUBLE x :Number) -> Number = (x)\n\
         TRY (DOUBLE \"hi\") -> :Str WITH (\
            Ok -> (PRINT \"ok\")\
            DispatchFailed -> (PRINT \"caught\")\
         )",
    );
    assert_eq!(bytes, b"caught\n");
}

#[test]
fn shape_error_arm_catches_shape_error() {
    // Inexhaustive MATCH is a deterministic ShapeError trigger.
    let bytes = run_program(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe (Some 1))\n\
         TRY (MATCH (m) -> :Number WITH (None -> (0))) -> :Str WITH (\
            ShapeError -> (PRINT it.message)\
         )",
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("inexhaustive"),
        "expected shape-error message about inexhaustive match, got {text:?}",
    );
}

#[test]
fn type_mismatch_arm_catches_record_newtype_value_mismatch() {
    // A record-repr newtype type-checks its value against the whole record repr, so the
    // mismatch names the record type rather than a single field type.
    let bytes = run_program(
        "NEWTYPE Point = :{x :Number, y :Number}\n\
         TRY (Point {x = \"hi\", y = 4}) -> :Str WITH (\
            TypeMismatch -> (PRINT it.expected)\
         )",
    );
    assert_eq!(bytes, b":{x :Number y :Number}\n");
}

#[test]
fn re_raise_when_no_arm_matches_error_kind() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("TRY (foo) -> :Str WITH (TypeMismatch -> (PRINT \"never\"))"),
    );
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "foo"),
        "expected re-raised UnboundName, got {err}",
    );
}

#[test]
fn missing_ok_arm_on_success_raises_shape_error() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("TRY (PRINT \"x\") -> :Str WITH (TypeMismatch -> (PRINT \"never\"))"),
    );
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("missing Ok arm")),
        "expected ShapeError about missing Ok arm, got {err}",
    );
}

#[test]
fn wildcard_arm_catches_when_no_specific_match() {
    let bytes = run_program(
        "TRY (foo) -> :Str WITH (\
            TypeMismatch -> (PRINT \"never\")\
            _ -> (PRINT \"caught wildcard\")\
         )",
    );
    assert_eq!(bytes, b"caught wildcard\n");
}

/// TRY body runs in a fresh `child_under` scope, so `LET x = 2` shadows rather
/// than rebinds the outer `x`. The WITH arm never fires.
#[test]
fn try_body_let_creates_local_binding_not_rebind() {
    let bytes = run_program(
        "LET x = 1\n\
         TRY (LET x = 2) -> :Str WITH (\
            _ -> (PRINT it.kind)\
         )",
    );
    assert!(
        bytes.is_empty(),
        "TRY body's LET should bind locally, not raise Rebind: got {:?}",
        String::from_utf8_lossy(&bytes),
    );
}

/// Companion: the TRY body's local LET must not survive past the TRY.
#[test]
fn try_body_let_not_visible_after_try() {
    let bytes = run_program(
        "LET x = 1\n\
         TRY (LET y = 99) -> :Str WITH (_ -> (PRINT it.kind))\n\
         PRINT x",
    );
    assert_eq!(bytes, b"1\n");
}

#[test]
fn specific_arm_wins_over_wildcard() {
    let bytes = run_program(
        "TRY (foo) -> :Str WITH (\
            _ -> (PRINT \"wildcard\")\
            UnboundName -> (PRINT \"specific\")\
         )",
    );
    assert_eq!(bytes, b"specific\n");
}

#[test]
fn frames_non_empty_after_recursive_call() {
    // PRINT renders a List as `[item, …]`, so a non-empty frames list starts
    // with `[in ` and an empty list is `[]`.
    let bytes = run_program(
        "FN (BAD n :Number) -> Any = (missing_name)\n\
         TRY (BAD 1) -> :Str WITH (\
            UnboundName -> (PRINT it.frames)\
         )",
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.starts_with("[in ") && text.contains("BAD"),
        "expected non-empty frames list naming BAD, got {text:?}",
    );
}

#[test]
fn nested_try_catches_inner_separately_from_outer() {
    let bytes = run_program(
        "NEWTYPE Point = :{x :Number, y :Number}\n\
         TRY (\
            TRY (Point {x = \"hi\", y = 4}) -> :Str WITH (\
                TypeMismatch -> (PRINT \"inner\")\
            )\
         ) -> :Str WITH (\
            Ok -> (PRINT \"outer ok\")\
         )",
    );
    assert_eq!(bytes, b"inner\nouter ok\n");
}

#[test]
fn it_resolves_via_scope_for_eval_of_top_level_quoted_reference() {
    // EVAL resolves names against the call-site scope at run time, so `it`
    // inside a top-level QUOTE only succeeds if the per-TRY child scope's `it`
    // binding is visible there.
    let bytes = run_program(
        "LET q = #(it)\n\
         TRY (PRINT \"value\") -> :Str WITH (\
            Ok -> (PRINT $(q))\
         )",
    );
    assert_eq!(bytes, b"value\nvalue\n");
}

#[test]
fn try_inside_tco_position_preserves_frame_chain() {
    // Mirror of `match_case::recursive_tagged_match_no_uaf`: the catch path
    // must keep the call-site frame Rc chained on the new frame.
    let bytes = run_program(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (TRY (MATCH (b) -> :Str WITH (\
            One -> (HOP (Bit (Zero null)))\
            Zero -> (PRINT \"done\")\
         )) -> :Str WITH (Ok -> it))\n\
         HOP (Bit (One null))",
    );
    assert_eq!(bytes, b"done\n");
}

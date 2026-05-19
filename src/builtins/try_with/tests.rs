//! TRY-WITH branch dispatch over success and per-`KErrorKind` arms, plus re-raise on
//! no-match and wildcard `_` coverage of dispatcher-internal kinds.

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent, run_root_with_buf};
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
    let bytes = run_program(
        "TRY (PRINT \"hello\") WITH (ok -> (PRINT \"caught ok\"))",
    );
    // The expr ran first (printing "hello") and the ok arm fires with `it` bound to
    // PRINT's return value ("hello"). The ok body prints "caught ok".
    assert_eq!(bytes, b"hello\ncaught ok\n");
}

#[test]
fn ok_binds_it_to_success_value() {
    let bytes = run_program(
        "TRY (PRINT \"value\") WITH (ok -> (PRINT it))",
    );
    // First PRINT writes "value" and returns "value"; `it` is bound to that string.
    assert_eq!(bytes, b"value\nvalue\n");
}

#[test]
fn unbound_name_arm_catches_unbound_name() {
    let bytes = run_program(
        "TRY (foo) WITH (\
            ok -> (PRINT \"ok\")\
            unbound_name -> (PRINT it.name)\
         )",
    );
    assert_eq!(bytes, b"foo\n");
}

#[test]
fn shape_error_arm_catches_shape_error() {
    // MATCH on a non-Tagged value raises ShapeError("inexhaustive match...") via
    // `branch_walk`. Use that as a deterministic trigger.
    let bytes = run_program(
        "UNION Maybe = (some :Number none :Null)\n\
         LET m = (Maybe (some 1))\n\
         TRY (MATCH (m) WITH (none -> (0))) WITH (\
            shape_error -> (PRINT it.message)\
         )",
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("inexhaustive"),
        "expected shape-error message about inexhaustive match, got {text:?}",
    );
}

#[test]
fn type_mismatch_arm_catches_struct_field_type_mismatch() {
    let bytes = run_program(
        "STRUCT Point = (x :Number, y :Number)\n\
         TRY (Point (x = \"hi\", y = 4)) WITH (\
            type_mismatch -> (PRINT it.expected)\
         )",
    );
    assert_eq!(bytes, b"Number\n");
}

#[test]
fn re_raise_when_no_arm_matches_error_kind() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("TRY (foo) WITH (type_mismatch -> (PRINT \"never\"))"),
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
        parse_one(
            "TRY (PRINT \"x\") WITH (type_mismatch -> (PRINT \"never\"))",
        ),
    );
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("missing ok arm")),
        "expected ShapeError about missing ok arm, got {err}",
    );
}

#[test]
fn wildcard_arm_catches_when_no_specific_match() {
    let bytes = run_program(
        "TRY (foo) WITH (\
            type_mismatch -> (PRINT \"never\")\
            _ -> (PRINT \"caught wildcard\")\
         )",
    );
    assert_eq!(bytes, b"caught wildcard\n");
}

#[test]
fn wildcard_catches_hidden_rebind_kind() {
    let bytes = run_program(
        "LET x = 1\n\
         TRY (LET x = 2) WITH (\
            _ -> (PRINT it.kind)\
         )",
    );
    assert_eq!(bytes, b"rebind\n");
}

#[test]
fn specific_arm_wins_over_wildcard() {
    let bytes = run_program(
        "TRY (foo) WITH (\
            _ -> (PRINT \"wildcard\")\
            unbound_name -> (PRINT \"specific\")\
         )",
    );
    assert_eq!(bytes, b"specific\n");
}

#[test]
fn frames_non_empty_after_recursive_call() {
    // A user-fn that always references an unbound name; the BAD call appends a frame
    // before the error reaches TRY's catch slot, so `it.frames` lists at least one
    // entry rendered as `"in <expression> (<function>)"`. PRINT a List renders as
    // `[item, …]`, so a non-empty list begins with `[in ` and an empty list is `[]`.
    let bytes = run_program(
        "FN (BAD n :Number) -> Any = (missing_name)\n\
         TRY (BAD 1) WITH (\
            unbound_name -> (PRINT it.frames)\
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
        "STRUCT Point = (x :Number, y :Number)\n\
         TRY (\
            TRY (Point (x = \"hi\", y = 4)) WITH (\
                type_mismatch -> (PRINT \"inner\")\
            )\
         ) WITH (\
            ok -> (PRINT \"outer ok\")\
         )",
    );
    assert_eq!(bytes, b"inner\nouter ok\n");
}

#[test]
fn it_resolves_via_scope_for_eval_of_top_level_quoted_reference() {
    // `substitute_params` rewrites every Identifier("it") in the branch body to a
    // Future at branch-dispatch time, so any direct mention of `it` works without a
    // scope-side binding. This test routes around substitution: `q` captures `(it)`
    // at the top level (where `it` is not in scope, but QUOTE doesn't evaluate), then
    // the branch body EVAL's `q`. EVAL resolves names against the call-site scope at
    // run time — only the per-TRY child scope's `it` binding can satisfy the lookup.
    // If that binding is removed, `it` is unbound when EVAL runs.
    let bytes = run_program(
        "LET q = #(it)\n\
         TRY (PRINT \"value\") WITH (\
            ok -> (PRINT $(q))\
         )",
    );
    assert_eq!(bytes, b"value\nvalue\n");
}

#[test]
fn try_inside_tco_position_preserves_frame_chain() {
    // TRY inside a recursive HOP through a tagged value. Mirrors
    // `match_case::recursive_tagged_match_no_uaf` — the catch path must keep the
    // call-site frame Rc chained on the new frame.
    let bytes = run_program(
        "UNION Bit = (one :Null zero :Null)\n\
         FN (HOP b :Tagged) -> Any = (TRY (MATCH (b) WITH (\
            one -> (HOP (Bit (zero null)))\
            zero -> (PRINT \"done\")\
         )) WITH (ok -> it))\n\
         HOP (Bit (one null))",
    );
    assert_eq!(bytes, b"done\n");
}

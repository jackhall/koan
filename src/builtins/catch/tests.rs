//! `CATCH <expr>` — lifting faults into `Result`, MATCH dispatch over the two variants,
//! non-short-circuiting in a binding position, nesting, and frame-chain preservation in
//! TCO position.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

fn run_program(source: &str) -> Vec<u8> {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}

#[test]
fn success_wraps_value_in_ok() {
    // `(PRINT "v")` prints "v" and returns "v"; CATCH wraps ok("v"); the ok arm's
    // `(PRINT it)` prints it again.
    let bytes = run_program(
        "MATCH (CATCH (PRINT \"v\")) WITH (ok -> (PRINT it) error -> (PRINT \"no\"))",
    );
    assert_eq!(bytes, b"v\nv\n");
}

#[test]
fn failure_wraps_to_tagged_in_error() {
    // The error payload is the raw `KError::to_tagged()` carrier, so MATCH-ing `it`
    // dispatches by `KErrorKind` tag; `unbound_name`'s payload carries `.name`.
    let bytes = run_program(
        "MATCH (CATCH (foo)) WITH (\
            ok -> (PRINT \"no\")\
            error -> (MATCH it WITH (unbound_name -> (PRINT it.name)))\
         )",
    );
    assert_eq!(bytes, b"foo\n");
}

#[test]
fn catch_in_let_does_not_short_circuit() {
    // A bare `LET r = (foo)` would fail the program; CATCH absorbs the fault so the
    // following statement still runs.
    let bytes = run_program(
        "LET r = (CATCH (foo))\n\
         (PRINT \"after\")",
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("after"), "expected program to continue, got {text:?}");
}

#[test]
fn nested_catch_wraps_inner_result_in_outer_ok() {
    // The inner CATCH *succeeds* (it produces a `Result` value rather than faulting),
    // so the outer CATCH wraps it in `ok`. `it` is then the inner `error(...)` Result.
    let bytes = run_program(
        "MATCH (CATCH (CATCH (foo))) WITH (\
            ok -> (MATCH it WITH (ok -> (PRINT \"inner-ok\") error -> (PRINT \"inner-error\")))\
            error -> (PRINT \"outer-error\")\
         )",
    );
    assert_eq!(bytes, b"inner-error\n");
}

#[test]
fn catch_inside_tco_position_preserves_frame_chain() {
    // CATCH inside a recursive HOP through a tagged value. Mirrors
    // `try_with::try_inside_tco_position_preserves_frame_chain` — the catch path must
    // keep the call-site frame Rc chained on the new frame.
    let bytes = run_program(
        "UNION Bit = (one :Null zero :Null)\n\
         FN (HOP b :Tagged) -> Any = (CATCH (MATCH (b) WITH (\
            one -> (HOP (Bit (zero null)))\
            zero -> (PRINT \"done\")\
         )))\n\
         HOP (Bit (one null))",
    );
    assert_eq!(bytes, b"done\n");
}

/// A CATCH-produced `Result` and a `Result (...)`-constructed `Result` share the carrier's
/// `(name, scope_id)` — the nominal identity that makes them MATCH the same way.
#[test]
fn catch_result_shares_identity_with_constructed_result() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let caught = run_one(scope, parse_one("CATCH (foo)"));
    let constructed = run_one(scope, parse_one("Result (ok 1)"));
    match (caught, constructed) {
        (
            KObject::Tagged { name: n1, scope_id: s1, .. },
            KObject::Tagged { name: n2, scope_id: s2, .. },
        ) => {
            assert_eq!(n1, "Result");
            assert_eq!(n1, n2);
            assert_eq!(s1, s2, "CATCH and constructed Result must share scope_id");
        }
        _ => panic!("expected both to be Tagged"),
    }
}

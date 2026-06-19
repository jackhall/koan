//! `CATCH <expr>` — lifting faults into `Result`, MATCH dispatch over the two variants,
//! non-short-circuiting in a binding position, nesting, and frame-chain preservation in
//! TCO position.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::model::KObject;
use crate::machine::KoanRegion;

fn run_program(source: &str) -> Vec<u8> {
    let arena = KoanRegion::new();
    let (scope, captured) = run_root_with_buf(&arena);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}

#[test]
fn success_wraps_value_in_ok() {
    // Double "v\n": PRINT both renders and returns its argument, so the ok
    // arm's `(PRINT it)` re-prints the same string CATCH captured.
    let bytes = run_program(
        "MATCH (CATCH (PRINT \"v\")) -> :Str WITH (Ok -> (PRINT it) Error -> (PRINT \"no\"))",
    );
    assert_eq!(bytes, b"v\nv\n");
}

#[test]
fn failure_wraps_to_tagged_in_error() {
    // Exercises the `KErrorKind`-tagged payload: nested MATCH dispatches on
    // the kind tag, and `.name` is the unbound_name variant's payload field.
    let bytes = run_program(
        "MATCH (CATCH (foo)) -> :Str WITH (\
            Ok -> (PRINT \"no\")\
            Error -> (MATCH it -> :Str WITH (UnboundName -> (PRINT it.name)))\
         )",
    );
    assert_eq!(bytes, b"foo\n");
}

#[test]
fn catch_in_let_does_not_short_circuit() {
    // Without CATCH the unbound `foo` would abort the program before the
    // second statement ran.
    let bytes = run_program(
        "LET r = (CATCH (foo))\n\
         (PRINT \"after\")",
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.contains("after"),
        "expected program to continue, got {text:?}"
    );
}

#[test]
fn nested_catch_wraps_inner_result_in_outer_ok() {
    // Inner CATCH *succeeds* (producing a Result), so the outer wraps it in
    // `ok`; `it` then names the inner `error(...)` Result.
    let bytes = run_program(
        "MATCH (CATCH (CATCH (foo))) -> :Str WITH (\
            Ok -> (MATCH it -> :Str WITH (Ok -> (PRINT \"inner-ok\") Error -> (PRINT \"inner-error\")))\
            Error -> (PRINT \"outer-error\")\
         )",
    );
    assert_eq!(bytes, b"inner-error\n");
}

#[test]
fn catch_inside_tco_position_preserves_frame_chain() {
    // Regression: the catch path must keep the call-site frame Rc chained on
    // the new frame across recursive HOPs, or the TCO continuation loses its
    // resumption context.
    let bytes = run_program(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (CATCH (MATCH (b) -> :Str WITH (\
            One -> (HOP (Bit (Zero null)))\
            Zero -> (PRINT \"done\")\
         )))\n\
         HOP (Bit (One null))",
    );
    assert_eq!(bytes, b"done\n");
}

/// Nominal identity: a CATCH-produced `Result` and a `Result (...)`-constructed one must
/// reference the *same* sealed `RecursiveSet` member so MATCH dispatches them identically.
#[test]
fn catch_result_shares_identity_with_constructed_result() {
    let arena = KoanRegion::new();
    let scope = run_root_silent(&arena);
    let caught = run_one(scope, parse_one("CATCH (foo)"));
    let constructed = run_one(scope, parse_one("Result (Ok 1)"));
    match (caught, constructed) {
        (
            KObject::Tagged {
                set: s1, index: i1, ..
            },
            KObject::Tagged {
                set: s2, index: i2, ..
            },
        ) => {
            assert_eq!(s1.member(*i1).name, "Result");
            assert!(
                std::rc::Rc::ptr_eq(s1, s2) && i1 == i2,
                "CATCH and constructed Result must share the same set member",
            );
        }
        _ => panic!("expected both to be Tagged"),
    }
}

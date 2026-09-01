//! `CATCH <expr>` — lifting faults into `Result`, MATCH dispatch over the two variants,
//! non-short-circuiting in a binding position, nesting, and frame-chain preservation in
//! TCO position.

use crate::builtins::test_support::{TestRun, type_name};
use crate::machine::model::{KObject, TypeNode};
use crate::machine::program_storage;
use crate::machine::run_root_storage;

fn run_program(source: &str) -> Vec<u8> {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(source);

    captured.borrow().clone()
}

#[test]
fn success_wraps_value_in_ok() {
    // Double "v\n": PRINT both renders and returns its argument, so the ok
    // arm's `(PRINT it)` re-prints the same string CATCH captured.
    let bytes = run_program(
        "MATCH (CATCH (PRINT \"v\")) OVER Result -> :Str WITH (Ok -> (PRINT it) Error -> (PRINT \"no\"))",
    );
    assert_eq!(bytes, b"v\nv\n");
}

/// A caught error is a `KError` member value, so the `Error` arm's `it` eliminates through the
/// `MATCH … OVER KError` member walk — and `_` defaults the kinds the arm set leaves out.
#[test]
fn failure_wraps_lowered_error_in_error() {
    let bytes = run_program(
        "MATCH (CATCH (foo)) OVER Result -> :Str WITH (\
            Ok -> (PRINT \"no\")\
            Error -> (MATCH it OVER KError -> :Str WITH (\
                UnboundName -> (PRINT it.name)\
                _ -> (PRINT \"other\")\
            ))\
         )",
    );
    assert_eq!(bytes, b"foo\n");
}

/// One identity spells the kind, so a caught error renders the variant name exactly once.
#[test]
fn caught_error_renders_its_kind_once() {
    let bytes = run_program("PRINT (CATCH (mystery))");
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        "Error(UnboundName({frames = [], name = mystery}))\n",
    );
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
        "MATCH (CATCH (CATCH (foo))) OVER Result -> :Str WITH (\
            Ok -> (MATCH it OVER Result -> :Str WITH (\
                Ok -> (PRINT \"inner-ok\") Error -> (PRINT \"inner-error\")))\
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
         FN (HOP b :Any) -> Any = (CATCH (MATCH (b) OVER Bit -> :Str WITH (\
            One -> (HOP (Bit.Zero null))\
            Zero -> (PRINT \"done\")\
         )))\n\
         HOP (Bit.One null)",
    );
    assert_eq!(bytes, b"done\n");
}

/// Nominal identity: a CATCH-produced `Error` and a `Result.Error`-constructed one must name the
/// *same* sealed member handle so the member walk selects them identically.
#[test]
fn catch_result_shares_identity_with_constructed_result() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let caught = test_run.run_one(test_run.parse_one("CATCH (foo)"));
    let constructed = test_run.run_one(test_run.parse_one("Result.Error 1"));
    match (caught, constructed) {
        (KObject::Wrapped { type_id: id1, .. }, KObject::Wrapped { type_id: id2, .. }) => {
            match test_run.types().node(*id1) {
                TypeNode::SetMember { name, .. } => {
                    assert_eq!(name, type_name("Error", test_run.registries()))
                }
                _ => panic!("expected a SetMember identity, got {id1:?}"),
            }
            assert_eq!(
                id1, id2,
                "CATCH and a constructed Error must name the same identity handle",
            );
        }
        _ => panic!("expected both to be Wrapped"),
    }
}

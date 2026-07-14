//! Runtime contract of the parse-static `#(...)` capture: an
//! [`ExpressionPart::QuotedExpression`](crate::machine::model::ast::ExpressionPart::QuotedExpression)
//! is a slot that behaves like a literal — a bare one rides the `LiteralPassThrough` lane to a
//! `KObject::KExpression`, an argument one binds that value without evaluating the body, and `$`
//! dispatches it back.

use crate::builtins::test_support::{run, run_root_with_buf};
use crate::machine::core::run_root_storage;

fn run_program(source: &str) -> Vec<u8> {
    let region = run_root_storage();
    let (scope, captured) = run_root_with_buf(&region);
    run(scope, source);
    let bytes = captured.borrow().clone();
    bytes
}

/// A single quoted part classifies `LiteralPassThrough`, so the expression's value *is* the
/// quoted body as data.
#[test]
fn bare_quote_evaluates_to_the_quoted_expression() {
    assert_eq!(run_program("PRINT #(1)"), b"1\n");
}

#[test]
fn let_binds_the_quoted_expression_without_evaluating_it() {
    // The body would print if it were evaluated at bind; only the EVAL run prints.
    assert_eq!(
        run_program("LET q = #(PRINT 1)\nPRINT \"bound\""),
        b"bound\n"
    );
    assert_eq!(run_program("LET q = #(PRINT 1)\n$(q)"), b"1\n");
}

/// `#(2)` reads the same in every continuation form the whitespace collapse produces.
#[test]
fn quote_round_trips_through_every_continuation_form() {
    assert_eq!(run_program("PRINT $(#(2))"), b"2\n");
    assert_eq!(run_program("LET q = #(2)\nPRINT $(q)"), b"2\n");
    assert_eq!(run_program("LET q =\n  #(2)\nPRINT $(q)"), b"2\n");
    // Bare `#2` parses only as a sigil-led line, where the indent collapse parenthesizes it.
    assert_eq!(run_program("LET q =\n  #2\nPRINT $(q)"), b"2\n");
}

/// A quote inside an aggregate literal rides its own one-part sub-dispatch (a `KExpression` has no
/// `'static` rebuild, so it cannot be a static cell), landing in the list as a value.
#[test]
fn quote_inside_a_list_literal_becomes_an_element_value() {
    assert_eq!(run_program("LET xs = [#(1) #(2)]\nPRINT xs"), b"[1, 2]\n");
}

/// A `:KExpression` parameter admits a quoted part and captures it raw — the lazy-candidate rule
/// that keeps the body from sub-dispatching before the callee sees it.
#[test]
fn kexpression_parameter_captures_a_quote_raw() {
    let bytes = run_program(
        "FN (KEEP q :KExpression) -> KExpression = (q)\n\
         LET kept = (KEEP #(PRINT 1))\n\
         PRINT \"held\"\n\
         $(kept)",
    );
    assert_eq!(bytes, b"held\n1\n");
}

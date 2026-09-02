//! Parsing the `-> Type` slot, and the runtime return-type check.

use crate::builtins::test_support::{TestRun, fn_is_registered, lookup_fn};
use crate::machine::KErrorKind;
use crate::machine::model::{KObject, KType, ReturnType};
use crate::machine::{program_storage, run_root_storage};
use crate::parse::parse;

#[test]
fn fn_parses_declared_return_type_onto_signature() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (DOUBLE x :Number) -> Number = (x)");

    let f = lookup_fn(scope, "DOUBLE");
    let ReturnType::Resolved(kt) = f.signature.return_type() else {
        panic!("declared return type should land resolved on the signature");
    };
    assert_eq!(kt, KType::NUMBER);
}

/// Missing `-> Type`: the FN call doesn't match the registered signature, so no user-fn
/// gets bound. Sub-expression dispatch may error first depending on body shape — the
/// load-bearing assertion is that `DOUBLE` isn't registered.
#[test]
fn fn_without_return_type_annotation_does_not_register() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "FN (DOUBLE x :Number) = (PRINT \"x\")",
    )
    .expect("parse should succeed");
    for expr in exprs {
        test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
            scope,
        );
    }
    let _ = test_run.runtime.execute();
    assert!(
        !fn_is_registered(scope, "DOUBLE"),
        "DOUBLE should not be registered without -> Type"
    );
}

/// Dispatch never selects on the return type, so two overloads whose shape and argument
/// types agree are indistinguishable at every call site no matter how their returns
/// differ — the second definition is a duplicate, not a new overload.
#[test]
fn return_type_only_difference_is_a_duplicate_overload() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (DOUBLE x :Number) -> Number = (x)"),
        ),
        scope,
    );
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (DOUBLE x :Number) -> Str = (\"a\")"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-node errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("return-type-only overload should be rejected as a duplicate"),
    };
    assert!(
        matches!(err.kind, KErrorKind::DuplicateOverload { ref name, .. } if name == "(DOUBLE _)"),
        "expected DuplicateOverload for (DOUBLE _), got {err}",
    );
}

#[test]
fn fn_with_unknown_return_type_name_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (DOUBLE x :Number) -> Bogus = (x)"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning 'Bogus', got {err}",
    );
}

#[test]
fn user_fn_return_type_mismatch_surfaces_as_kerror() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (LIE) -> Number = (\"oops\")");
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("LIE"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("LIE should fail return-type check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("LIE")),
        "expected a frame naming the call site LIE, got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
    assert!(
        err.frames
            .iter()
            .any(|f| f.expression.contains(":(FN :{} -> Number)")),
        "the same frame carries the callable's by-name identity, got {:?}",
        err.frames.iter().map(|f| &f.expression).collect::<Vec<_>>(),
    );
}

/// User-bound type alias as a FN return type: elaborates against the captured scope.
#[test]
fn fn_with_user_bound_return_type_works() {
    use super::capture_program_output;
    let bytes = capture_program_output(
        "LET MyT = Number\n\
         FN (DOIT xs :MyT) -> MyT = (xs)\n\
         PRINT (DOIT 7)",
    );
    assert_eq!(bytes, b"7\n");
}

/// A FN's parameter and return-type slots referencing a name (`MyT`) bound by a *later*
/// statement is a forward reference — the ratified H2 ruling: structural use of a
/// lexically-later name is a resolution error, not a park. `MyT`'s `LET` sits at a higher
/// lexical index than the FN, so the FN's sigil elaboration finds no visible `MyT` and
/// surfaces a `ShapeError` naming it.
#[test]
fn fn_return_type_forward_user_bound_name_is_a_resolution_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let edges: Vec<_> = parse(
        program.brand(),
        &test_run.registries().labels,
        "FN (DOIT xs :MyT) -> MyT = (xs)\nLET MyT = Number",
    )
    .expect("parse succeeds")
    .into_iter()
    .map(|e| {
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        );
        test_run.runtime.install_edge_for_test(id, scope)
    })
    .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edges[0]) {
        Err(e) => e,
        Ok(()) => panic!("FN referencing the later-bound MyT should fail to resolve"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("MyT")),
        "expected ShapeError naming the forward type MyT, got {err}",
    );
}

/// Pins the surface-form-survives-bind guarantee on `KObject::TypeNameRef` —
/// see [ktype/slots-and-signatures.md § TypeNameRef](../../../../design/typing/ktype/slots-and-signatures.md#ktypeunresolved--surface-form-survives-bind).
#[test]
fn fn_return_type_surface_name_preserved_in_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (DOIT) -> SomeWeirdName = (1)"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("SomeWeirdName")),
        "expected ShapeError mentioning 'SomeWeirdName' verbatim, got {err}",
    );
}

#[test]
fn user_fn_with_any_return_type_accepts_anything() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (PURE) -> Any = (\"a string\")");
    let result = test_run.run_one(test_run.parse_one("PURE"));
    assert!(matches!(result, KObject::KString(s) if *s == "a string"));
}

/// Keep-first across a cross-function tail chain: `OUTER`'s declared `-> Number` governs the whole
/// chain, so a violation introduced only by the chain's *final* tail value still errors against
/// `OUTER`'s contract — and the error's trace frame names `OUTER` (the first call), not the inner
/// callee that produced the offending value. `MIDDLE` and `INNER` declare `-> Any` (FN registration
/// requires a `-> Type`), so their own contracts would *accept* the `Str`; the mismatch fires only
/// because keep-first keeps `OUTER`'s `-> Number` across both hops (`OUTER -> MIDDLE -> INNER`) and
/// carries its retained frame, rendered at error time from `OUTER`'s call site and signature. This
/// exercises the invoke-continue/redispatch keep-first over a two-deep cross-function chain, not
/// self-recursion.
#[test]
fn keep_first_across_tail_chain_errors_against_outer_contract() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (INNER) -> Any = (\"nope\")");
    test_run.run("FN (MIDDLE) -> Any = (INNER)");
    test_run.run("FN (OUTER) -> Number = (MIDDLE)");
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("OUTER"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => {
            panic!(
                "OUTER should fail: the chain's final tail returns a Str against OUTER's -> Number"
            )
        }
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(
                expected, "Number",
                "the kept-first contract is OUTER's -> Number, not the callees' -> Any",
            );
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("OUTER")),
        "the kept-first contract's frame names the first call site, OUTER, got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
    assert!(
        err.frames
            .iter()
            .any(|f| f.expression.contains(":(FN :{} -> Number)")),
        "the frame's by-name identity is OUTER's own signature, got {:?}",
        err.frames.iter().map(|f| &f.expression).collect::<Vec<_>>(),
    );
    assert!(
        !err.frames.iter().any(|f| f.expression.contains("-> Any")),
        "no callee's -> Any signature reaches the frame — keep-first from both directions, got {:?}",
        err.frames.iter().map(|f| &f.expression).collect::<Vec<_>>(),
    );
}

/// A tail-spliced declared-return obligation is discharged before any consumer reads the rehomed
/// terminal. `WRAP`'s body tail is a bare name (`x`) bound by a preceding `LET`; statement-at-a-time
/// submission puts every statement in flight before any of them executes, so `x` can still be a
/// submit-time placeholder when the body decides — the slot splices out via `Outcome::Forward` (an
/// already-*bound* name would read as a plain `Done`, never a forward) rather than parking a
/// continuation. `WRAP`'s `-> Number` obligation rides the splice, so before the forwarded terminal
/// reaches the `out` consumer the checker discharges the declared return against the producer's
/// value — here through the parked-checker micro-step, since `x`'s producer is unresolved when the
/// consumer decides. A non-matching `Str` fires the mismatch at the splice check; a matching `Number`
/// forwards through intact.
#[test]
fn spliced_bare_name_tail_checks_declared_return() {
    // Non-matching: the bare-name tail forwards a Str; the splice check rejects it against -> Number.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let bad_edges: Vec<_> = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET x = \"nope\"\nFN (WRAP) -> Number = (x)\nLET out = (WRAP)",
    )
    .expect("parse succeeds")
    .into_iter()
    .map(|e| {
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        );
        test_run.runtime.install_edge_for_test(id, scope)
    })
    .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(bad_edges[2]) {
        Err(e) => e,
        Ok(()) => panic!("the spliced Str tail must fail WRAP's -> Number check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return> from the splice check, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("WRAP")),
        "the splice check frames the mismatch at the obligation's call site (WRAP), got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
    assert!(
        err.frames
            .iter()
            .any(|f| f.expression.contains(":(FN :{} -> Number)")),
        "and carries WRAP's by-name identity beside it, got {:?}",
        err.frames.iter().map(|f| &f.expression).collect::<Vec<_>>(),
    );

    // Matching: the bare-name tail forwards a Number; the splice check passes and the value arrives.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let ok_edges: Vec<_> = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET x = 7\nFN (WRAP) -> Number = (x)\nLET out = (WRAP)",
    )
    .expect("parse succeeds")
    .into_iter()
    .map(|e| {
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        );
        test_run.runtime.install_edge_for_test(id, scope)
    })
    .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        test_run.runtime.edge_result_error(ok_edges[2]).is_ok(),
        "the matching spliced value passes the splice check: {:?}",
        test_run.runtime.edge_result_error(ok_edges[2]).err(),
    );
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0),
        "the matching spliced value forwards through intact to out",
    );
}

/// A return type naming a type whose binder is still in flight parks on that producer and
/// re-resolves at wake off the token's own symbol: the capture carries the parsed name across the
/// park, so the woken elaboration runs against the wake-side scope with the name it started with.
#[test]
fn fn_return_type_parks_on_an_in_flight_binder_and_resolves_at_wake() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let statements: Vec<_> = parse(
        program.brand(),
        &test_run.registries().labels,
        "NEWTYPE Later = Number\nFN (WRAPIT x :Number) -> Later = (x)",
    )
    .expect("parse succeeds")
    .into_iter()
    .map(|e| crate::machine::model::WorkingExpression::from_ast(scope.brand(), e))
    .collect();
    test_run.runtime.enter_block(scope.id, statements, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");

    let f = lookup_fn(scope, "WRAPIT");
    let ReturnType::Resolved(kt) = f.signature.return_type() else {
        panic!("the parked return type resolves by the time the FN registers");
    };
    assert_eq!(
        kt,
        crate::builtins::test_support::lookup_type(scope, "Later")
            .expect("the NEWTYPE binds its member"),
        "the woken elaboration lands the same handle the NEWTYPE sealed",
    );
}

/// The return slot is one union carrier, so a bare `Type` token reaching it is a raw capture the
/// body resolves — including a `LET` alias of a builtin leaf, which no builtin-table lowering would
/// have found.
#[test]
fn fn_return_type_resolves_a_let_alias_of_a_builtin_leaf() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run
        .run("LET MyNum = Number\nFN (DOUBLE x :Number) -> MyNum = (x * 2)\nLET out = (DOUBLE 3)");

    let f = lookup_fn(scope, "DOUBLE");
    let ReturnType::Resolved(kt) = f.signature.return_type() else {
        panic!("an aliased return type resolves at definition time");
    };
    assert_eq!(kt, KType::NUMBER, "the alias lands the leaf it names");
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 6.0));
}

/// A `:{…}` return rides the union's `RecordType` member — captured raw, re-wrapped as the
/// single-part node that folds to its record type, and still enforced against the body's result.
#[test]
fn fn_record_return_type_is_captured_and_enforced() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (WRAPIT x :Number) -> :{v :Number} = ({v = x})\nLET out = (WRAPIT 3)");
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Record(..))),
        "the record return elaborates and the call lands a record",
    );

    let mut second = TestRun::silent(&program, &region);
    let scope = second.scope;
    second.run("FN (UNWRAPIT x :Number) -> :{v :Number} = (x)");
    let id = second.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            second.parse_one("UNWRAPIT 3"),
        ),
        scope,
    );
    let edge = second.runtime.install_edge_for_test(id, scope);
    second
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        second.runtime.edge_result_error(edge).is_err(),
        "a Number body must not satisfy a `:{{v :Number}}` return",
    );
}

/// The union's `IDENTIFIER` member carries the diagnose-only arm that used to be its own overload:
/// a lowercase return name is still met with the pointed `TYPE OF` suggestion, not a dispatch miss.
#[test]
fn fn_value_named_return_stays_pointed() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (DOUBLE elem :Number) -> elem = (elem)"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("a value-named return should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("names a type, but `elem` is a value")
                && msg.contains("-> :(TYPE OF elem)")),
        "expected the pointed value-named-return error, got {err}",
    );
}

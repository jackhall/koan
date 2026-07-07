//! End-to-end coverage for the bare-name short-circuit, auto-wrap pass, and
//! replay-park routing in `classify_dispatch` (see
//! [design/execution/name-placeholders.md § Dispatch-time name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders)).
use crate::builtins::default_scope;
use crate::machine::core::run_root_storage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{KObject, KType};
use crate::machine::KErrorKind;
use crate::parse::parse;

fn parse_one<'run>(src: &str) -> crate::machine::model::ast::KExpression<'run> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

fn parse_all<'run>(src: &str) -> Vec<crate::machine::model::ast::KExpression<'run>> {
    parse(src).expect("parse should succeed")
}

#[test]
fn single_identifier_short_circuit_returns_value_when_bound() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all("LET x = 42") {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    let id = runtime.dispatch_in_scope(parse_one("(x)"), scope);
    runtime.execute().unwrap();
    assert!(runtime
        .read_result_with(
            id,
            |v| matches!(v.object(), KObject::Number(n) if *n == 42.0)
        )
        .expect("value"));
}

/// Index-gated LET visibility — see [design/execution/README.md § Dispatch-time
/// name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn single_identifier_short_circuit_value_let_forward_ref_is_unbound() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids = runtime.enter_block(scope.id, parse_all("LET y = (x)\nLET x = 1"), scope);
    runtime.execute().unwrap();
    let err = runtime
        .result_error(ids[0])
        .err()
        .cloned()
        .expect("forward-ref LET should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "x"),
        "expected UnboundName('x'), got {err}",
    );
}

#[test]
fn single_identifier_short_circuit_falls_through_when_unbound() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("(missing)"), scope);
    runtime.execute().unwrap();
    let err = match runtime.result_error(id) {
        Err(e) => e.clone(),
        Ok(()) => panic!("missing should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "missing"),
        "expected UnboundName, got {err}",
    );
}

#[test]
fn bare_identifier_in_value_slot_auto_wraps_and_resolves() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all("LET z = 7\nLET y = z") {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
}

/// Wrap-slot companion of the LET forward-ref test: the eager-name resolve must
/// surface `UnboundName` under the gate, not park on the later-sibling binding.
#[test]
fn bare_identifier_in_value_slot_forward_ref_is_unbound() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids = runtime.enter_block(scope.id, parse_all("LET y = z\nLET z = 9"), scope);
    runtime.execute().unwrap();
    let err = runtime
        .result_error(ids[0])
        .err()
        .cloned()
        .expect("forward-ref wrap-slot should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "z"),
        "expected UnboundName('z'), got {err}",
    );
}

/// Backward-ref shape: producers precede the consumer so the gate doesn't hide
/// them, and the multi-producer wrap-slot replay-park wakes once both finalize.
#[test]
fn multiple_value_slot_placeholders_park_on_distinct_producers() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (a)\n\
         LET aa = 3\n\
         LET bb = 4\n\
         LET out = (ADD aa BY bb)",
    ) {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
}

/// FN is value-style gated — see [design/execution/README.md § Dispatch-time
/// name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn forward_keyword_function_reference_is_unbound() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids = runtime.enter_block(
        scope.id,
        parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x :Number) -> Number = (x)",
        ),
        scope,
    );
    runtime
        .execute()
        .expect("a forward-FN dispatch failure is slot-terminal");
    let err = runtime
        .result_error(ids[0])
        .expect_err("forward-FN call should fail dispatch");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_)
        ),
        "expected DispatchFailed or UnboundName, got {err}",
    );
}

#[test]
fn multi_producer_replay_park_waits_for_all_then_re_dispatches() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET aa = 11\n\
         LET bb = 22\n\
         LET out = (ADD aa BY bb)",
    ) {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
}

/// Miri audit-slate: bare-name forward-splice lifetime contract — see
/// [design/execution/README.md § Miri forward-splice and replay-park lifetime
/// contract](../../../../design/execution/name-placeholders.md#miri-forward-splice-and-replay-park-lifetime-contract).
#[test]
fn lift_park_minimal_program_for_miri() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all("LET z = 11\nLET y = z") {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 11.0));
}

/// Miri audit-slate: pins the replay-park scope-lifetime contract — the parked
/// slot's scope must stay valid across the wake and the re-dispatch.
#[test]
fn replay_park_minimal_program_for_miri() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all(
        "FN (DOUBLE x :Number) -> Number = (x)\n\
         LET aa = 7\n\
         LET out = (DOUBLE aa)",
    ) {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
}

/// A producer that errors at dispatch time finalizes its slot with the error
/// (slot-terminal); the consumer parked on it inherits the error rather than
/// `execute` aborting.
#[test]
fn replay_park_propagates_producer_error() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let ids: Vec<_> = parse_all(
        "LET y = (x)\n\
         LET x = (UNDEFINED_FN)",
    )
    .into_iter()
    .map(|e| runtime.dispatch_in_scope(e, scope))
    .collect();
    runtime
        .execute()
        .expect("a producer error routes into the slot, not a fatal execute abort");
    assert!(
        runtime.result_error(ids[1]).is_err(),
        "the UNDEFINED_FN producer call must error",
    );
    assert!(
        runtime.result_error(ids[0]).is_err(),
        "y must inherit its dependency's error",
    );
    assert!(
        scope.lookup("y").is_none(),
        "y should not bind when its dependency errors"
    );
}

/// Bare Type-tokens in `ProperType` slots of non-binders ride the same
/// replay-park rails as bare Identifiers — see
/// [design/execution/name-placeholders.md § Dispatch-time name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn bare_type_token_in_typeexprref_slot_parks_when_forward_referenced() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    for e in parse_all(
        "LET AResult = (IntOrd :| OrderedSig)\n\
         MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    ) {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(
        matches!(
            scope.resolve_type("AResult"),
            Some(KType::Module { module: _ })
        ),
        "AResult should bind to a Module identity (type-only) after replay-park on \
         forward-declared MODULE / SIG",
    );
}

/// Language invariant: a type value never binds to a value-classified
/// (lowercase-leading) identifier. `LET ty = Number` is rejected; the
/// Type-classified `LET Ty = Number` is the legal way to alias a type.
/// (`Ty` rather than `T` because single-letter uppercase tokens don't classify
/// as Type names.)
#[test]
fn let_type_to_value_name_rejected() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("LET ty = Number"), scope);
    runtime.execute().unwrap();
    match runtime.read_result_with(id, |v| format!("{:?}", v.ktype())) {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg)
                if msg.contains("ty") && msg.contains("Type-classified")),
            "expected a value-classified-type rejection, got {e}",
        ),
        Ok(ktype) => panic!("LET ty = Number must be rejected, got {ktype}"),
    }

    // The Type-classified alias is the legal form: it lands type-side.
    let mut runtime = KoanRuntime::new();
    for e in parse_all("LET Ty = Number") {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert_eq!(scope.resolve_type("Ty"), Some(&KType::Number));
}

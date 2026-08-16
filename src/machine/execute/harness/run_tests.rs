//! End-to-end coverage for the bare-name short-circuit, auto-wrap pass, and
//! replay-park routing in `classify_dispatch` (see
//! [design/execution/name-placeholders.md § Dispatch-time name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders)).
use crate::builtins::test_support::TestRun;
use crate::builtins::test_support::binds_module;
use crate::machine::KErrorKind;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::{KObject, KType};

use super::tests::{working_all as parse_all, working_one as parse_one};

#[test]
fn single_identifier_short_circuit_returns_value_when_bound() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for e in parse_all(&program, "LET x = 42") {
        runtime.dispatch_in_scope(e, scope, 1);
    }
    runtime.execute().unwrap();
    let id = runtime.dispatch_in_scope(parse_one(&program, "(x)"), scope, 2);
    let edge = runtime.install_edge_for_test(id, scope);
    runtime.execute().unwrap();
    assert!(
        runtime
            .read_edge_result_with(
                edge,
                |v| matches!(v.object(), KObject::Number(n) if *n == 42.0)
            )
            .expect("value")
    );
}

/// Index-gated LET visibility — see [design/execution/README.md § Dispatch-time
/// name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn single_identifier_short_circuit_value_let_forward_ref_is_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(
        scope.id,
        parse_all(&program, "LET y = (x)\nLET x = 1"),
        scope,
    );
    let edge = runtime.install_edge_for_test(ids[0], scope);
    runtime.execute().unwrap();
    let err = runtime
        .edge_result_error(edge)
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(parse_one(&program, "(missing)"), scope, 1);
    let edge = runtime.install_edge_for_test(id, scope);
    runtime.execute().unwrap();
    let err = match runtime.edge_result_error(edge) {
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for (i, e) in parse_all(&program, "LET z = 7\nLET y = z")
        .into_iter()
        .enumerate()
    {
        runtime.dispatch_in_scope(e, scope, i + 1);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
}

/// Wrap-slot companion of the LET forward-ref test: the eager-name resolve must
/// surface `UnboundName` under the gate, not park on the later-sibling binding.
#[test]
fn bare_identifier_in_value_slot_forward_ref_is_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(scope.id, parse_all(&program, "LET y = z\nLET z = 9"), scope);
    let watched = super::tests::watch_all(runtime, &ids, scope);
    runtime.execute().unwrap();
    let err = runtime
        .edge_result_error(watched[0])
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for (i, e) in parse_all(
        &program,
        "FN (ADD a :Number BY b :Number) -> Number = (a)\n\
         LET aa = 3\n\
         LET bb = 4\n\
         LET out = (ADD aa BY bb)",
    )
    .into_iter()
    .enumerate()
    {
        runtime.dispatch_in_scope(e, scope, i + 1);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
}

/// FN is value-style gated — see [design/execution/README.md § Dispatch-time
/// name placeholders](../../../../design/execution/name-placeholders.md#dispatch-time-name-placeholders).
#[test]
fn forward_keyword_function_reference_is_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(
        scope.id,
        parse_all(
            &program,
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x :Number) -> Number = (x)",
        ),
        scope,
    );
    let edge = runtime.install_edge_for_test(ids[0], scope);
    runtime
        .execute()
        .expect("a forward-FN dispatch failure is slot-terminal");
    let err = runtime
        .edge_result_error(edge)
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for (i, e) in parse_all(
        &program,
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET aa = 11\n\
         LET bb = 22\n\
         LET out = (ADD aa BY bb)",
    )
    .into_iter()
    .enumerate()
    {
        runtime.dispatch_in_scope(e, scope, i + 1);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
}

/// Miri audit-slate: both park lifetime contracts in one batch-submitted program — see
/// [design/execution/README.md § Miri forward-splice and replay-park lifetime
/// contract](../../../../design/execution/name-placeholders.md#miri-forward-splice-and-replay-park-lifetime-contract).
/// `LET y = z` forward-splices a bare name whose producer has not run yet (the lift park), and
/// `LET out = (DOUBLE y)` parks a FN call on that same binding and replays it on the wake — the
/// parked slot's scope must stay valid across both the wake and the re-dispatch, which is
/// `Scheduler::replace` / `NodeStore::reinstall` re-projecting the slot's scope from the frame cart.
#[test]
fn park_and_replay_minimal_program_for_miri() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for (i, e) in parse_all(
        &program,
        "LET z = 11\n\
         LET y = z\n\
         FN (DOUBLE x :Number) -> Number = (x)\n\
         LET out = (DOUBLE y)",
    )
    .into_iter()
    .enumerate()
    {
        runtime.dispatch_in_scope(e, scope, i + 1);
    }
    runtime.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 11.0));
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 11.0));
}

/// A producer that errors at dispatch time finalizes its slot with the error
/// (slot-terminal); the consumer parked on it inherits the error rather than
/// `execute` aborting.
#[test]
fn replay_park_propagates_producer_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    let ids: Vec<_> = parse_all(
        &program,
        "LET y = (x)\n\
         LET x = (UNDEFINED_FN)",
    )
    .into_iter()
    .enumerate()
    .map(|(i, e)| runtime.dispatch_in_scope(e, scope, i + 1))
    .collect();
    let edges: Vec<_> = ids
        .iter()
        .map(|&id| runtime.install_edge_for_test(id, scope))
        .collect();
    runtime
        .execute()
        .expect("a producer error routes into the slot, not a fatal execute abort");
    assert!(
        runtime.edge_result_error(edges[1]).is_err(),
        "the UNDEFINED_FN producer call must error",
    );
    assert!(
        runtime.edge_result_error(edges[0]).is_err(),
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
/// All three statements are submitted before any of them runs, so `a_result`'s consumer can still
/// reach the scheduler ahead of the MODULE / SIG binders it depends on — its own index makes both
/// visible, but their slots may still be finalizing when it dispatches, so it parks and replays
/// once they're done.
#[test]
fn bare_type_token_in_typeexprref_slot_parks_while_still_finalizing() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let runtime = &mut test_run.runtime;
    for (i, e) in parse_all(
        &program,
        "MODULE int_ord = (LET compare = 0)\n\
         SIG Ordered = (VAL compare :Number)\n\
         LET a_result = (int_ord :| Ordered)",
    )
    .into_iter()
    .enumerate()
    {
        runtime.dispatch_in_scope(e, scope, i + 1);
    }
    runtime.execute().unwrap();
    assert!(
        binds_module(scope, "a_result"),
        "a_result should bind to the ascribed module value after parking on \
         the still-finalizing MODULE / SIG binders",
    );
}

/// Language invariant: a type value never binds to a value-classified
/// (lowercase-leading) identifier. `LET ty = Number` is rejected; the
/// Type-classified `LET Ty = Number` is the legal way to alias a type.
/// (`Ty` rather than `T` because single-letter uppercase tokens don't classify
/// as Type names.)
#[test]
fn let_type_to_value_name_rejected() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let watch = test_run.dispatch_watched_in(scope, parse_one(&program, "LET ty = Number"));
    test_run.runtime.execute().unwrap();
    let types = test_run.types.clone();
    match test_run
        .runtime
        .read_edge_result_with(watch, |v| format!("{:?}", v.ktype(&types)))
    {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg)
                if msg.contains("ty") && msg.contains("Type-classified")),
            "expected a value-classified-type rejection, got {e}",
        ),
        Ok(ktype) => panic!("LET ty = Number must be rejected, got {ktype}"),
    }

    // The Type-classified alias is the legal form: it lands type-side.
    for e in parse_all(&program, "LET Ty = Number") {
        test_run.dispatch_in_scope(e, scope);
    }
    test_run.runtime.execute().unwrap();
    assert_eq!(scope.resolve_type("Ty"), Some(KType::NUMBER));
}

//! End-to-end coverage for the bare-name short-circuit, auto-wrap pass, and
//! replay-park routing in `run_dispatch` (see
//! [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)).
use super::Scheduler;
use crate::builtins::default_scope;
use crate::machine::model::{KObject, KType};
use crate::machine::SchedulerHandle;
use crate::machine::{KErrorKind, RuntimeArena};
use crate::parse::parse;

fn parse_one<'a>(src: &str) -> crate::machine::model::ast::KExpression<'a> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

fn parse_all<'a>(src: &str) -> Vec<crate::machine::model::ast::KExpression<'a>> {
    parse(src).expect("parse should succeed")
}

#[test]
fn single_identifier_short_circuit_returns_value_when_bound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET x = 42") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let id = sched.add_dispatch(parse_one("(x)"), scope);
    sched.execute().unwrap();
    assert!(matches!(sched.read(id), KObject::Number(n) if *n == 42.0));
}

/// Index-gated LET visibility — see [design/execution-model.md § Dispatch-time
/// name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders).
#[test]
fn single_identifier_short_circuit_value_let_forward_ref_is_unbound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(scope.id, parse_all("LET y = (x)\nLET x = 1"), scope);
    sched.execute().unwrap();
    let err = sched
        .read_result(ids[0])
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
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("(missing)"), scope);
    sched.execute().unwrap();
    let err = match sched.read_result(id) {
        Err(e) => e.clone(),
        Ok(_) => panic!("missing should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "missing"),
        "expected UnboundName, got {err}",
    );
}

#[test]
fn bare_identifier_in_value_slot_auto_wraps_and_resolves() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET z = 7\nLET y = z") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
}

/// Wrap-slot companion of the LET forward-ref test: the eager-name resolve must
/// surface `UnboundName` under the gate, not park on the later-sibling binding.
#[test]
fn bare_identifier_in_value_slot_forward_ref_is_unbound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(scope.id, parse_all("LET y = z\nLET z = 9"), scope);
    sched.execute().unwrap();
    let err = sched
        .read_result(ids[0])
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
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (a)\n\
         LET aa = 3\n\
         LET bb = 4\n\
         LET out = (ADD aa BY bb)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
}

/// FN is value-style gated — see [design/execution-model.md § Dispatch-time
/// name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders).
#[test]
fn forward_keyword_function_reference_is_unbound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    sched.enter_block(
        scope.id,
        parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x :Number) -> Number = (x)",
        ),
        scope,
    );
    let err = sched
        .execute()
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
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET aa = 11\n\
         LET bb = 22\n\
         LET out = (ADD aa BY bb)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
}

/// Miri audit-slate: Lift-park lifetime contract — see [design/execution-model.md
/// § Miri Lift-park lifetime contract](../../../../design/execution-model.md#miri-lift-park-lifetime-contract).
#[test]
fn lift_park_minimal_program_for_miri() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET z = 11\nLET y = z") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 11.0));
}

/// Miri audit-slate: pins the replay-park scope-lifetime contract — the parked
/// slot's scope must stay valid across the wake and the re-dispatch.
#[test]
fn replay_park_minimal_program_for_miri() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "FN (DOUBLE x :Number) -> Number = (x)\n\
         LET aa = 7\n\
         LET out = (DOUBLE aa)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
}

/// A producer that errors at dispatch time aborts `execute` via `?` propagation
/// rather than routing the failure into the consumer's slot.
#[test]
fn replay_park_propagates_producer_error() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "LET y = (x)\n\
         LET x = (UNDEFINED_FN)",
    ) {
        sched.add_dispatch(e, scope);
    }
    let exec_result = sched.execute();
    assert!(
        exec_result.is_err(),
        "UNDEFINED_FN dispatch failure should surface via execute",
    );
    assert!(
        scope.lookup("y").is_none(),
        "y should not bind when its dependency errors"
    );
}

/// Bare Type-tokens in `TypeExprRef` slots of non-binders ride the same
/// replay-park rails as bare Identifiers — see
/// [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders).
#[test]
fn bare_type_token_in_typeexprref_slot_parks_when_forward_referenced() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "LET AResult = (IntOrd :| OrderedSig)\n\
         MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(
        matches!(
            scope.resolve_type("AResult"),
            Some(KType::Module {
                module: _,
                frame: _
            })
        ),
        "AResult should bind to a Module identity (type-only) after replay-park on \
         forward-declared MODULE / SIG",
    );
}

/// `LET ty = Number` must bind `ty` to a `KTypeValue` carrying the `Number`
/// leaf — the wrap → `BareTypeLeaf` fast lane has to match the literal
/// `KTypeValue` carve-out observably. (Lowercase LHS because single-letter
/// uppercase tokens don't classify as Type names.)
#[test]
fn let_t_equals_number_still_binds_ktype_value() {
    use crate::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET ty = Number") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    match scope.lookup("ty") {
        Some(KObject::KTypeValue(t)) => {
            assert_eq!(*t, KType::Number);
        }
        other => panic!(
            "ty should bind to KTypeValue(Number), got {:?}",
            other.map(|o| o.ktype())
        ),
    }
}

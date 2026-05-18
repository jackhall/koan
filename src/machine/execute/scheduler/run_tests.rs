//! End-to-end coverage for the bare-name short-circuit, auto-wrap pass, and
//! replay-park routing in `run_dispatch` (see
//! [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)).
use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::{KErrorKind, RuntimeArena};
use super::Scheduler;
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

/// Submission order matters: `LET y = (x)` dispatches first and parks on `x`'s
/// placeholder; `LET x = 1` then wakes the parked sub.
#[test]
fn single_identifier_short_circuit_lift_parks_on_placeholder() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET y = (x)\nLET x = 1") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
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

#[test]
fn bare_identifier_in_value_slot_parks_when_forward_referenced() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET y = z\nLET z = 9") {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 9.0));
}

#[test]
fn multiple_value_slot_placeholders_park_on_distinct_producers() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (a)\n\
         LET out = (ADD aa BY bb)\n\
         LET aa = 3\n\
         LET bb = 4",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
}

/// The FN binder skips the placeholder install when the name is already a function in
/// scope (overload model), so the callee must not yet be in `data` when the caller
/// dispatches for a true forward-reference park.
#[test]
fn call_by_name_replay_parks_on_forward_function_reference() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "LET out = (DOUBLE 7)\n\
         FN (DOUBLE x :Number) -> Number = (x)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
}

#[test]
fn multi_producer_replay_park_waits_for_all_then_re_dispatches() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET out = (ADD aa BY bb)\n\
         LET aa = 11\n\
         LET bb = 22",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
}

/// Miri audit-slate: pins the bare-name-short-circuit Lift-park lifetime contract. The `&KObject<'a>` the
/// Lift returns is the producer's reference, not a clone — the arena must outlive
/// the wake and re-run.
#[test]
fn lift_park_minimal_program_for_miri() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all("LET y = z\nLET z = 11") {
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
        "LET out = (DOUBLE 7)\n\
         FN (DOUBLE x :Number) -> Number = (x)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
}

/// A producer that errors at dispatch time aborts `execute` via `?` propagation.
/// Rerouting sub-Dispatch failures into the consumer's slot is a follow-up.
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
    assert!(scope.lookup("y").is_none(), "y should not bind when its dependency errors");
}

/// A bare Type-token in a `TypeExprRef` slot of a non-binder picks up the same
/// replay-park rails as a bare Identifier: `IntOrd :| OrderedSig` submitted before
/// `MODULE IntOrd` / `SIG OrderedSig` must park on the placeholders the binders install
/// rather than racing the FIFO submission order. Pins the Type-token park symmetry
/// described in
/// [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders).
#[test]
fn bare_type_token_in_typeexprref_slot_parks_when_forward_referenced() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    for e in parse_all(
        "LET aResult = (IntOrd :| OrderedSig)\n\
         MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(
        matches!(scope.lookup("aResult"), Some(KObject::KModule(_, _))),
        "aResult should bind to a KModule after replay-park on forward-declared MODULE / SIG",
    );
}

/// Substrate cross-check: `LET ty = Number` still binds `ty` to a `KTypeValue`
/// carrying the `Number` leaf. After the unification, the value flows through the
/// wrap → `value_lookup`-TypeExprRef path rather than the literal `KTypeValue`
/// carve-out; the observable binding must be identical to the literal path. (Lowercase
/// LHS because single-letter uppercase tokens don't classify as Type names.)
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

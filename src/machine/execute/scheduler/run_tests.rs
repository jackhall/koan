//! End-to-end coverage for the bare-name short-circuit, auto-wrap pass, and
//! replay-park routing in `run_dispatch` (see
//! [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)).
use crate::builtins::default_scope;
use crate::machine::SchedulerHandle;
use crate::machine::model::{KObject, KType};
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

/// Under index-gated resolution a later-sibling LET is invisible to an earlier
/// sibling's reference — `LET y = (x)` at index `i` cannot see `LET x = 1` at
/// index `i+1` (the strict `b.idx < c` predicate hides the producer's binding,
/// and LET is value-style gated). The consumer surfaces `UnboundName`.
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

/// Index-gated companion: a bare-Identifier wrap-slot reference whose binding is
/// declared at a later sibling is invisible under the gate. The wrap-slot's
/// eager-name resolve surfaces `UnboundName` rather than parking and waking when
/// the later sibling finalizes.
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

/// Backward-ref companion: putting the LET binders ahead of the consumer keeps the
/// multi-producer wrap-slot replay-park alive under the gate — the producers'
/// indices sit strictly less than the consumer's, so the gate doesn't hide them
/// and the placeholder/park mechanism still wakes the slot once both finalize.
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

/// Under index-gated resolution a forward call to a later-sibling FN is invisible
/// to the consumer (FN is value-style gated, not a nominal binder). The
/// dispatch's per-scope `Bindings::lookup_function` filters by the same
/// visibility predicate (and the `pending_overloads` fall-through it covers
/// inherits the same gate), so the call surfaces `DispatchFailed` rather than
/// parking on the not-yet-finalized overload — `execute` returns `Err`
/// directly (matching the `?` propagation on a dispatch miss; see
/// `Scheduler::run_dispatch`).
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
    let err = sched.execute().expect_err("forward-FN call should fail dispatch");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_)),
        "expected DispatchFailed or UnboundName, got {err}",
    );
}

/// Backward-ref companion exercising the multi-producer replay-park wake — the
/// LET binders for `aa`/`bb` precede the consumer, so the gate doesn't hide
/// them, and the placeholder/park mechanism still wakes the slot once both
/// finalize.
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

/// Miri audit-slate: pins the bare-name-short-circuit Lift-park lifetime contract.
/// The `&KObject<'a>` the Lift returns is the producer's reference, not a clone —
/// the arena must outlive the wake and re-run. Under index-gated resolution the
/// consumer must sit *after* the producer (otherwise the gate hides the binding),
/// so this is a backward-ref shape; the parking mechanism still exercises the
/// Lift/notify lifetimes when a same-block producer hasn't terminalized at the
/// consumer's submit time.
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
/// slot's scope must stay valid across the wake and the re-dispatch. Backward-ref
/// shape (FN-decl before call) keeps the call dispatchable under the gate; the
/// call's wrap-slot may still replay-park (e.g. on an `aa` placeholder) when the
/// arg is a not-yet-bound name, which is what exercises the lifetime contract.
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
        "LET AResult = (IntOrd :| OrderedSig)\n\
         MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    ) {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    assert!(
        matches!(scope.lookup("AResult"), Some(KObject::KTypeValue(KType::Module { module: _, frame: _ }))),
        "AResult should bind to a KModule after replay-park on forward-declared MODULE / SIG",
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

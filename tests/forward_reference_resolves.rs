//! Integration tests for the dispatch-time-placeholders feature: a binder dispatched
//! before its reference can be resolved by parking the consumer on the producer's slot
//! via the scheduler's `notify_list` / `pending_deps` machinery. Covers the §1
//! single-Identifier short-circuit and the §8 replay-park for forward function-name
//! references, exercised end-to-end via `interpret_with_writer` and the public scheduler
//! API.

use std::cell::RefCell;
use std::rc::Rc;

use koan::runtime::builtins::default_scope;
use koan::runtime::machine::model::KObject;
use koan::runtime::machine::{RuntimeArena, Scheduler, Scope};
use koan::parse::parse;

/// Scaffolding: spin up a fresh arena + default scope, run `source` end-to-end through
/// the scheduler, and return both the captured PRINT output and the root scope so tests
/// can assert on bindings post-run.
fn run<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>, source: &str) -> &'a Scope<'a> {
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let scope = default_scope(arena, Box::new(SharedBuf(captured)));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs { sched.add_dispatch(e, scope); }
    sched.execute().expect("scheduler should run to completion");
    scope
}

/// Reverse-order LET: `LET y = z` then `LET z = 1`. The §1 short-circuit on the
/// auto-wrapped `z` parks on the binder's placeholder; once `LET z = 1` finalizes, the
/// notify-walk wakes the parked Lift and `y` ends up bound to `1`.
#[test]
fn reverse_order_let_resolves_via_placeholder() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "LET y = z\nLET z = 1");
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// MODULE-body forward reference: a member that references another member declared later
/// in the same module body. Each statement dispatches against the module's child scope
/// via a fresh inner scheduler; the forward-reference parking applies inside that scope.
#[test]
fn module_body_forward_reference_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "MODULE Mod = ((LET y = x) (LET x = 1))");
    let m = match scope.lookup("Mod") {
        Some(KObject::KModule(m, _)) => *m,
        _ => panic!("Mod should be a module"),
    };
    let data = m.child_scope().bindings().data();
    assert!(matches!(data.get("y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// Multi-name in one expression: a single value-slot expression whose RHS references two
/// not-yet-bound names. §7 wraps each, §8 / §1 park each on its respective producer; the
/// outer slot resumes once both finalize.
#[test]
fn multi_name_forward_reference_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    // `ADD a BY b` returns `b` so the test reads the second forward reference's value.
    let scope = run(
        &arena,
        captured,
        "FN (ADD a: Number BY b: Number) -> Number = (b)\n\
         LET out = (ADD aa BY bb)\n\
         LET aa = 1\n\
         LET bb = 2",
    );
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 2.0));
}

/// Forward function-name reference (call_by_name): `f x` before `FN f x: Number ->
/// Number`. The call_by_name slot picks up `f` as an Identifier reference; §8 parks the
/// outer slot on `f`'s placeholder. When FN finalizes, the call resumes and dispatches.
#[test]
fn forward_call_by_name_resolves_after_fn_definition() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "LET out = (DOUBLE 5)\n\
         FN (DOUBLE x: Number) -> Number = (x)",
    );
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 5.0));
}

/// Forward struct-name reference (ATTR `s.x`): a struct-typed `LET p = (Pt 3 4)` followed
/// by `LET v = p.x`. Submitted in reverse order — `LET v = p.x` first, then the
/// constructor — so the lookup parks on `p`'s placeholder.
///
/// Today the v1 conservative-park may also park on `x` if a binder named `x` were in
/// flight (it isn't here), so the test asserts only the success path.
#[test]
fn forward_attr_lookup_resolves_after_struct_binding() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "LET v = p.x\n\
         STRUCT Pt = (x: Number, y: Number)\n\
         LET p = (Pt (x: 7, y: 9))",
    );
    assert!(matches!(scope.lookup("v"), Some(KObject::Number(n)) if *n == 7.0));
}

/// Forward LET type alias: `LET Ty = Un; LET Un = Number`. The Type-classed `Un` token
/// on the RHS of the first LET parks on `Un`'s dispatch-time placeholder, resumes when
/// `LET Un = Number` finalizes, and ends with `Ty` resolving to `Number` via the
/// `bindings.types` chain.
#[test]
fn forward_let_type_alias_resolves_to_number() {
    use koan::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "LET Ty = Un\nLET Un = Number");
    assert!(
        matches!(scope.resolve_type("Ty"), Some(KType::Number)),
        "expected Ty to resolve to Number, got {:?}",
        scope.resolve_type("Ty"),
    );
}

/// Module-qualified type name in LET-RHS position. `LET MyT = Mo.Ty` where `Mo` is a
/// module exporting `Ty = Number`. The RHS auto-wraps and sub-Dispatches the ATTR; the
/// chain produces a `KTypeValue(kt)` via `access_module_member`'s `Scope::resolve_type`
/// fallback (stage 1.7 routed module-body `LET Ty = ...` through `register_type` into
/// `bindings.types`, which `access_module_member` consults), and the LET-TypeExprRef-LHS
/// overload routes that carrier through `register_type` on the parent scope.
#[test]
fn let_alias_via_module_qualified_type_resolves() {
    use koan::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "MODULE Mo = ((LET Ty = Number))\nLET MyT = Mo.Ty",
    );
    assert!(
        matches!(scope.resolve_type("MyT"), Some(KType::Number)),
        "expected MyT to resolve to Number via Mo.Ty, got {:?}",
        scope.resolve_type("MyT").map(|t| t.name()),
    );
}

/// Module-qualified type name in FN parameter (and return) position. `Mo.Ty` arrives
/// as `ExpressionPart::Expression` in the FN signature; `parse_fn_param_list` records
/// it for sub-Dispatch and splices back the resolved type as `Future(KTypeValue)` on
/// the Combine wake. The return-type slot's `Expression([ATTR Mo Ty])` rides the
/// tentative-pass `Expression`-in-non-`KExpression`-slot allowance on
/// `KFunction::accepts_for_wrap`, then `lazy_eager_indices` puts the slot into
/// `eager_indices` so the dispatcher schedules its sub-Dispatch. The subsequent
/// `LET y = (ID 7)` parks on `ID`'s dispatch-time placeholder via the head-Keyword
/// fallback in `run_dispatch`'s `Unmatched` arm — without that, the call would race
/// FN's Combine-deferred registration and surface as `no matching function`.
#[test]
fn fn_param_with_module_qualified_type_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "MODULE Mo = ((LET Ty = Number))\n\
         FN (ID x: Mo.Ty) -> Mo.Ty = (x)\n\
         LET y = (ID 7)",
    );
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
}

/// Module-qualified type name in a `LIST_OF`-style type frame. `LIST_OF Mo.Ty` rides
/// the existing `Deferred` path in `resolve_dispatch`: the bare `LIST_OF` overload
/// (`elem: TypeExprRef`) rejects the `Expression` part on strict match, but
/// `expr_has_eager_part` returns true so `schedule_eager_fallthrough` sub-Dispatches
/// `Mo.Ty` and re-binds with `Future(KTypeValue(_))` in the slot.
#[test]
fn type_frame_with_module_qualified_element_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "MODULE Mo = ((LET Ty = Number))\n\
         LET MyList = (LIST_OF Mo.Ty)",
    );
    assert!(
        scope.resolve_type("MyList").is_some(),
        "expected MyList to bind via LIST_OF Mo.Ty",
    );
}

/// Chained module-qualified type name `Outer.Inner.T`. The inner `Outer.Inner` resolves
/// via `access_module_member` to the inner module's `KObject::KModule` value (preferring
/// the `bindings.data` arm so the chain stays drillable); the outer `.T` then hits the
/// inner module's `bindings.types` via the same helper's `resolve_type` fallback and
/// surfaces as `KTypeValue(Number)`. Pins that the value-side ATTR walker produces a
/// usable `KTypeValue` for type-position consumers across the chain.
#[test]
fn chained_module_qualified_type_resolves() {
    use koan::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "MODULE Outer = ((MODULE Inner = ((LET Ty = Number))))\n\
         LET MyT = Outer.Inner.Ty",
    );
    assert!(
        matches!(scope.resolve_type("MyT"), Some(KType::Number)),
        "expected MyT to resolve to Number via Outer.Inner.Ty, got {:?}",
        scope.resolve_type("MyT"),
    );
}

/// Producer-error propagation: when a forward reference's producer errors at dispatch
/// time (e.g. `LET x = (UNDEFINED_FN)` — the inner expression has no matching function),
/// `Scheduler::execute` returns the dispatch failure directly. The consumer's slot may
/// not finalize because execute aborts on the first `?` propagation; the assertion is
/// that the run surfaces the structured error.
///
/// This is the existing dispatch-failure path; the new placeholder machinery doesn't
/// change it. A future cycle-detection / structured-error follow-up may switch this to
/// an in-band `Err` on the consumer's slot — that's tracked as an open question on the
/// roadmap item.
#[test]
fn producer_error_propagates_to_parked_consumer() {
    use koan::runtime::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let scope = default_scope(&arena, Box::new(SharedBuf(captured.clone())));
    let exprs = parse(
        "LET y = (x)\n\
         LET x = (UNDEFINED_FN)",
    ).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs { let _ = sched.add_dispatch(e, scope); }
    let exec_result = sched.execute();
    let err = exec_result.expect_err("execute should surface UNDEFINED_FN's dispatch failure");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for UNDEFINED_FN, got {err}",
    );
}


//! Integration tests for the dispatch-time-placeholders feature under the index-gated
//! resolution rule. Forward references at the same lexical level are no longer parked
//! through to a later sibling: the strict `b.idx < c` visibility predicate hides
//! later-sibling bindings from earlier consumers, and only nominal binders (STRUCT,
//! named UNION, SIG, FUNCTOR, MODULE) carry the D7 carve-out that re-exposes them to
//! siblings on the same block. This file pins both shapes — the value-style
//! `UnboundName` surface for LET / FN forward references, and the nominal-binder
//! continued-resolution path.

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::{KObject, KType, Parseable};
use koan::machine::{RuntimeArena, Scheduler, SchedulerHandle, Scope};
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
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let scope = default_scope(arena, Box::new(SharedBuf(captured)));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    sched.enter_block(scope.id, exprs, scope);
    let _ = sched.execute();
    scope
}

/// Run `source`, returning the first errored top-level slot's error (or `None` if every
/// slot succeeded). Pairs with the new `UnboundName`-surfacing tests below.
fn run_collecting_first_err(source: &str) -> Option<koan::machine::KError> {
    let arena = RuntimeArena::new();
    struct Sink;
    impl std::io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    let ids: Vec<_> = sched.enter_block(scope.id, exprs, scope);
    if let Err(e) = sched.execute() {
        return Some(e);
    }
    for id in ids {
        if let Err(e) = sched.read_result(id) {
            return Some(e.clone());
        }
    }
    None
}

/// Forward LET at the same lexical level is `UnboundName`: a value LET's binding sits at
/// its statement's index, which is strictly greater than any earlier consumer's cutoff,
/// and LET is not a nominal-binder carve-out. The eager wrap-resolve / short-circuit
/// passes both surface `UnboundName(z)` from the perspective of `LET y = z`.
#[test]
fn forward_value_let_at_same_level_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("LET y = z\nLET z = 1")
        .expect("forward value LET reference should surface UnboundName");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z'), got {err}",
    );
}

/// Backward-ref companion: swapping the order so the producer precedes the consumer
/// re-enables the placeholder + park machinery. The bind at the earlier index satisfies
/// `b.idx < c` for the consumer's cutoff, the consumer's wrap-resolve either resolves
/// directly or parks on the live placeholder, and the slot wakes when `LET z` finalizes.
#[test]
fn backward_value_let_at_same_level_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "LET z = 1\nLET y = z");
    assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
}

/// MODULE body: forward LET reference under the body's child scope is `UnboundName` for
/// the same reason as the top-level case — `LET y = x` sits at body-scope index `i`,
/// `LET x = 1` at index `i+1`, and the gate hides the producer from the consumer.
#[test]
fn module_body_forward_value_reference_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("MODULE Mod = ((LET y = x) (LET x = 1))")
        .expect("forward value LET in module body should surface UnboundName");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "x"),
        "expected UnboundName('x'), got {err}",
    );
}

/// Module-body backward-ref companion: with the producer ahead of the consumer the
/// resolution succeeds normally.
#[test]
fn module_body_backward_value_reference_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "MODULE Mod = ((LET x = 1) (LET y = x))");
    // MODULE is type-only — the `&Module` rides the identity in `types`.
    let m = match scope.resolve_type("Mod") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("Mod should be a module identity in types"),
    };
    let y = m.child_scope().lookup("y");
    assert!(matches!(y, Some(KObject::Number(n)) if *n == 1.0));
}

/// Multi-name in one expression: a single value-slot expression whose RHS references two
/// not-yet-bound names is `UnboundName` (the first unbound argument wins on the error
/// surface). Pinned to confirm the wrap-slot pass propagates the gate uniformly across
/// every bare-name part rather than only the first.
#[test]
fn multi_name_forward_reference_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET out = (ADD aa BY bb)\n\
         LET aa = 1\n\
         LET bb = 2",
    )
    .expect("forward refs in FN call should surface UnboundName");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::UnboundName(_) | KErrorKind::DispatchFailed { .. }
        ),
        "expected UnboundName or DispatchFailed, got {err}",
    );
}

/// Backward-ref companion: ordering the LET binders ahead of the consumer makes both
/// references visible under the gate and the call resolves normally.
#[test]
fn multi_name_backward_reference_resolves() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "FN (ADD a :Number BY b :Number) -> Number = (b)\n\
         LET aa = 1\n\
         LET bb = 2\n\
         LET out = (ADD aa BY bb)",
    );
    assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 2.0));
}

/// Forward function-name reference (call_by_name): a later-sibling `FN DOUBLE` is
/// value-style gated (FN is not a nominal-binder carve-out), so `LET out = (DOUBLE 5)`
/// at an earlier index cannot see the binder. The dispatch's per-scope
/// `Bindings::lookup_function` filters by the same visibility predicate and
/// surfaces `DispatchFailed` (or `UnboundName` depending on how far the dispatch
/// reaches before the gate fires).
#[test]
fn forward_call_by_name_is_dispatch_failure() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "LET out = (DOUBLE 5)\n\
         FN (DOUBLE x :Number) -> Number = (x)",
    )
    .expect("forward FN call should surface a dispatch error");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::DispatchFailed { .. } | KErrorKind::UnboundName(_)
        ),
        "expected DispatchFailed or UnboundName, got {err}",
    );
}

/// Forward struct-name reference (ATTR `s.x`): the value LET `p` is invisible to the
/// earlier consumer under the gate even though `STRUCT Pt` itself is a nominal-binder
/// carve-out. The carve-out covers the type identity, not the value-side LET that
/// constructs an instance.
#[test]
fn forward_attr_lookup_through_value_let_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err(
        "LET v = p.x\n\
         STRUCT Pt = (x :Number, y :Number)\n\
         LET p = (Pt {x = 7, y = 9})",
    )
    .expect("forward ATTR on value LET should surface UnboundName");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::UnboundName(_) | KErrorKind::DispatchFailed { .. }
        ),
        "expected UnboundName or DispatchFailed, got {err}",
    );
}

/// Backward-ref companion: the value LET precedes its consumer. The STRUCT identity is
/// already a nominal binder visible to siblings; the value-side carrier `p` sits at an
/// earlier index than `v`, so both references resolve.
#[test]
fn backward_attr_lookup_resolves_after_struct_binding() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "STRUCT Pt = (x :Number, y :Number)\n\
         LET p = (Pt {x = 7, y = 9})\n\
         LET v = p.x",
    );
    assert!(matches!(scope.lookup("v"), Some(KObject::Number(n)) if *n == 7.0));
}

/// LET-as-type-alias is value-style gated: `LET Ty = Un` followed by `LET Un = Number`
/// hides `Un` from `Ty`'s consumer under the strict cutoff, so the elaborator surfaces
/// `UnboundName` rather than resolving forward through the placeholder. (Type-side
/// resolution applies the same gate as value-side; see `Scope::resolve_type_with_chain`.)
#[test]
fn forward_let_type_alias_is_unbound() {
    use koan::machine::KErrorKind;
    let err = run_collecting_first_err("LET Ty = Un\nLET Un = Number")
        .expect("forward LET type alias should surface UnboundName");
    assert!(
        matches!(
            &err.kind,
            KErrorKind::UnboundName(_) | KErrorKind::DispatchFailed { .. }
        ),
        "expected UnboundName or DispatchFailed, got {err}",
    );
}

/// Backward-ref companion for the type-alias case: ordering the alias's target first
/// makes the LET-Ty resolve to `Number` via the gated `resolve_type_with_chain` walk.
#[test]
fn backward_let_type_alias_resolves_to_number() {
    use koan::machine::model::KType;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(&arena, captured, "LET Un = Number\nLET Ty = Un");
    assert!(
        matches!(scope.resolve_type("Ty"), Some(KType::Number)),
        "expected Ty to resolve to Number, got {:?}",
        scope.resolve_type("Ty"),
    );
}

/// Module-qualified type name in LET-RHS position. `LET MyT = Mo.Ty` where `Mo` is a
/// module exporting `Ty = Number`. MODULE is a nominal-binder carve-out, so `Mo` is
/// visible to its sibling consumer regardless of source order; the inner module-body
/// LET runs once `Mo` finalizes, the ATTR walker reads its `bindings.types`, and the
/// LET-Type-LHS overload routes the carrier through `register_type` on the parent
/// scope.
#[test]
fn let_alias_via_module_qualified_type_resolves() {
    use koan::machine::model::KType;
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

/// Module-qualified type name in a `LIST_OF`-style type frame. `LIST_OF Mo.Ty` rides
/// the existing `Deferred` path in `resolve_dispatch`. MODULE Mo is a nominal-binder
/// carve-out so it's visible to the sibling `LET MyList`.
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

/// Chained module-qualified type name `Outer.Inner.T`. Both modules are nominal-binder
/// carve-outs and visible regardless of source order.
#[test]
fn chained_module_qualified_type_resolves() {
    use koan::machine::model::KType;
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

/// Producer-error propagation: when a consumer references a still-pending producer (now
/// only possible across nominal-binder carve-outs or backward references), an error at
/// the producer propagates through. With value LETs gated, the simplest backward shape
/// — `LET x = (UNDEFINED_FN)` first, then `LET y = (x)` — keeps the visibility test
/// satisfied while exercising the error-propagation rails.
#[test]
fn producer_error_propagates_to_parked_consumer() {
    use koan::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);
    impl std::io::Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let scope = default_scope(&arena, Box::new(SharedBuf(captured.clone())));
    let exprs = parse(
        "LET x = (UNDEFINED_FN)\n\
         LET y = (x)",
    )
    .expect("parse should succeed");
    let mut sched = Scheduler::new();
    for e in exprs {
        let _ = sched.add_dispatch(e, scope);
    }
    let exec_result = sched.execute();
    let err = exec_result.expect_err("execute should surface UNDEFINED_FN's dispatch failure");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for UNDEFINED_FN, got {err}",
    );
}

/// Bucket-keyed FN park: a bare-arg call to a still-finalizing FN whose signature
/// parameter is a STRUCT (nominal-binder carve-out, visible across siblings). The FN
/// itself is value-style gated, so the call must come *after* the FN's submission to
/// satisfy the visibility predicate; the bucket-keyed park then carries it through
/// the FN's elaboration on the STRUCT placeholder.
///
/// Submission order:
///   1. `FN (LIFT_BARE arg :Wrap) -> Number = (7)` — installs a
///      `pending_overloads[{Keyword("LIFT_BARE"), Slot}] = NodeId(this binder)`
///      entry via the bucket-keyed `binder_bucket` hook. `Wrap` (the param type)
///      is a forward reference to the STRUCT below — visible because STRUCT is a
///      nominal-binder carve-out.
///   2. `STRUCT Wrap = (n :Number)`.
///   3. `LET w = (Wrap {n = 9})`.
///   4. `LET out = (LIFT_BARE w)`.
#[test]
fn fn_bare_arg_call_parks_on_pending_overload_bucket() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        &arena,
        captured,
        "FN (LIFT_BARE arg :Wrap) -> Number = (7)\n\
         STRUCT Wrap = (n :Number)\n\
         LET w = (Wrap {n = 9})\n\
         LET out = (LIFT_BARE w)",
    );
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0),
        "expected `out` to be 7.0 via bucket-keyed FN park; got {}",
        scope
            .lookup("out")
            .map_or("None".to_string(), |o| o.summarize()),
    );
}

/// Self-referential binding guard: `LET x = x` is the degenerate "same-lexical-index"
/// case — the producer's `x` placeholder sits at index `i`, the RHS consumer reads at
/// cutoff `i`, and the strict `b.idx < c` predicate makes the binding invisible. The
/// consumer surfaces `UnboundName`. (The pre-gate path emitted `SchedulerDeadlock` via
/// `would_create_cycle`; the gate now intercepts the self-reference earlier.)
#[test]
fn self_referential_binding_is_unbound() {
    use koan::machine::interpret_with_writer;
    use koan::machine::KErrorKind;
    let err = interpret_with_writer("LET x = x", Box::new(std::io::sink()))
        .expect_err("self-reference should error");
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "x"),
        "expected UnboundName('x'), got {err}",
    );
}

//! PR C: cache-driven strict-only dispatch pipeline. Each test pins one path
//! through `run_dispatch`'s rewritten body:
//!
//! - Strict admit picks via a cached `NameOutcome::Resolved` (the bare-name's
//!   carrier type satisfies the slot's `KType`).
//! - Post-walk fallback surfaces `ParkOnProducers` / `UnboundName` / `Deferred`
//!   from the cache's `Parked` / `Unbound` outcomes — precedence:
//!   placeholders > eager > unbound > pending > unmatched.
//! - Upfront `ProducerErrored` sweep propagates a dep's terminal error before
//!   the candidate walk.
//! - Cycle detection runs in the fused splice/park walk on wrap / ref-name
//!   slots; the cache itself is built with `consumer = None`.
//!
//! Tests use a hidden output buffer so PRINT noise doesn't pollute stderr.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::builtins::default_scope;
use crate::machine::execute::Scheduler;
use crate::machine::SchedulerHandle;
use crate::machine::{KErrorKind, RuntimeArena};
use crate::parse::parse;

struct Sink;
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Strict admit reads the cache: a bare-name arg pre-bound to a Number value
/// satisfies a `:Number` slot via `accepts_part(Future(...))`, with no
/// tentative-pass fall-through. Pinned by capturing the PRINT output.
#[test]
fn strict_pick_via_cached_value_admits_on_carrier_type() {
    let arena = RuntimeArena::new();
    let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(&arena, Box::new(SharedBuf(Rc::clone(&buf))));
    let exprs = parse("LET x = 7\nPRINT x").expect("parse should succeed");
    let mut sched = Scheduler::new();
    sched.enter_block(scope.id, exprs, scope);
    sched.execute().expect("two-statement program should run");
    let captured = buf.borrow().clone();
    assert_eq!(captured, b"7\n", "PRINT x should output the bound value");
}

/// Strict-Empty fallback: a bare-name arg in a value-slot resolves to a
/// not-yet-bound forward reference (`LET y = z\nLET z = 1`). LET admits via
/// shape-only fallback on the Unbound outcome; the fused splice/park walk's
/// wrap-slot logic surfaces `UnboundName("z")` as a slot terminal at the
/// wrap site. The pre-PR-C `classify_tie_bare_names` path is gone, but the
/// surface is preserved by reading the cache directly during the splice walk.
#[test]
fn strict_empty_unbound_surfaces_via_wrap_slot_terminal() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse("LET y = z\nLET z = 1").expect("parse should succeed");
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(scope.id, exprs, scope);
    let _ = sched.execute();
    // Index 0 is `LET y = z` — should surface UnboundName('z').
    let err = match sched.read_result(ids[0]) {
        Err(e) => e.clone(),
        Ok(_) => panic!("forward reference to later LET should surface UnboundName"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "z"),
        "expected UnboundName('z') from the wrap-slot terminal, got {err}",
    );
}

/// Self-reference `LET Ty = Ty`: under index-gated resolution the consumer
/// sees its own placeholder as hidden (`b.idx < c` fails since both are at
/// the same index, value LET binders aren't nominal). Cache has
/// `Unbound("Ty")`; LET admits shape-only; the fused walk's wrap-slot reads
/// `Unbound` and surfaces `UnboundName("Ty")`. Cycle detection in the walk
/// (`would_create_cycle` on Parked outcomes) only fires when the placeholder
/// is *visible* and points back at this slot — a separate code path.
#[test]
fn self_referential_let_surfaces_unbound_name() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse("LET Ty = Ty").expect("parse should succeed");
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(scope.id, exprs, scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(ids[0]) {
        Err(e) => e.clone(),
        Ok(_) => panic!("self-referential LET should surface UnboundName"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n == "Ty"),
        "expected UnboundName('Ty') from the wrap-slot terminal, got {err}",
    );
}

/// Bare-name forward reference to a placeholder produces `Parked(producer)`
/// in the cache. LET admits shape-only on the Parked outcome (its value slot
/// is `KType::Any`; the Identifier-decl slot is exempted). The fused walk's
/// wrap-slot arm pushes the producer onto `producers_to_wait` and installs a
/// combined park; on wake the rebuilt cache sees `Resolved(_)` and the dispatch
/// commits.
#[test]
fn forward_reference_parks_then_resolves_on_wake() {
    let arena = RuntimeArena::new();
    let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(&arena, Box::new(SharedBuf(Rc::clone(&buf))));
    // `LET y = x` parks on `x`'s placeholder (nominal binder MODULE carves out
    // the visibility gate; here we use STRUCT to get the same nominal-binder
    // semantics for the placeholder).
    let exprs = parse(
        "STRUCT Foo = (x :Number)\n\
         LET fwd = Foo\n\
         PRINT fwd",
    )
    .expect("parse should succeed");
    let mut sched = Scheduler::new();
    sched.enter_block(scope.id, exprs, scope);
    sched.execute().expect("dispatch with bare-name park should complete");
    let captured = buf.borrow().clone();
    // PRINT renders the StructType carrier; just assert that something
    // printed, since the exact rendering is not load-bearing here.
    assert!(
        !captured.is_empty(),
        "PRINT fwd should produce output after the forward reference resolves",
    );
}

//! Cache-driven strict-only dispatch surface tests not covered elsewhere:
//! self-reference `LET Ty = Ty` (cache `Unbound`, wrap-slot terminalizes
//! without entering cycle detection) and bare-name forward reference to a
//! nominal-binder placeholder (cache `Parked`, splice walk installs combined
//! park, slot commits on wake).

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

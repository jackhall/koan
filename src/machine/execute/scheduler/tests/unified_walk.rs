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

/// Self-reference `LET Ty = Ty`: the consumer sees its own placeholder as
/// hidden under index-gating (same idx, LET binders aren't nominal), so the
/// cache holds `Unbound("Ty")` and the wrap-slot terminal surfaces
/// `UnboundName`. Cycle detection only fires on visible Parked outcomes — a
/// separate path.
#[test]
fn self_referential_let_surfaces_unbound_name() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let exprs = parse("LET Ty = Ty").expect("parse should succeed");
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(scope.id, exprs, scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(ids[0]) {
        Err(e) => e.clone(),
        Ok(_) => panic!("self-referential LET should surface UnboundName"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n.contains("Ty")),
        "expected UnboundName naming Ty from the wrap-slot terminal, got {err}",
    );
}

/// Bare-name forward reference to a placeholder: cache holds
/// `Parked(producer)`, LET admits shape-only, the wrap-slot installs a
/// combined park, and on wake the rebuilt cache resolves and dispatch commits.
#[test]
fn forward_reference_parks_then_resolves_on_wake() {
    let arena = RuntimeArena::new();
    let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(&arena, Box::new(SharedBuf(Rc::clone(&buf))));
    // STRUCT (like MODULE) is a nominal binder, so the placeholder is visible
    // to the forward reference and parks rather than reading as Unbound.
    let exprs = parse(
        "NEWTYPE Foo = :{x :Number}\n\
         LET Fwd = Foo\n\
         PRINT Fwd",
    )
    .expect("parse should succeed");
    let mut sched = Scheduler::new();
    sched.enter_block(scope.id, exprs, scope);
    sched
        .execute()
        .expect("dispatch with bare-name park should complete");
    let captured = buf.borrow().clone();
    // `Fwd` aliases the struct's type identity (Type-classified name); exact
    // rendering of that type value isn't load-bearing here.
    assert!(
        !captured.is_empty(),
        "PRINT Fwd should produce output after the forward reference resolves",
    );
}

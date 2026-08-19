//! Cache-driven strict-only dispatch surface: self-reference `LET Ty = Ty`
//! (cache `Unbound`) and a bare-name reference to a still-pending binder
//! placeholder (cache `Parked`, park installed at dispatch, slot commits on
//! wake).

use super::working_all;
use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::core::{program_storage, run_root_storage};

/// Self-reference `LET Ty = Ty`: index gating is a strict `idx <` predicate, so
/// the in-progress binding is invisible to its own RHS at the same idx. The
/// cache holds `Unbound("Ty")`, which resolution reports as a dead lean and
/// `keyworded::initial` surfaces as `UnboundName` rather than self-parking.
#[test]
fn self_referential_let_surfaces_unbound_name() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = working_all(&program, "LET Ty = Ty");
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(scope.id, exprs, scope);
    let watched = super::watch_all(runtime, &ids, scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.edge_result_error(watched[0]) {
        Err(e) => e.clone(),
        Ok(()) => panic!("self-referential LET should surface UnboundName"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(n) if n.contains("Ty")),
        "expected UnboundName naming Ty from the dead-lean terminal, got {err}",
    );
}

/// Bare-name reference to a still-finalizing binder: the cache holds
/// `Parked(producer)`, LET's binder slot admits shape-only, and on wake the
/// rebuilt cache resolves and dispatch commits.
#[test]
fn pending_producer_parks_then_resolves_on_wake() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, buf) = TestRun::with_buf(&program, &region);
    let scope = test_run.scope;
    // `Foo` is declared in an earlier statement, so it is index-visible to
    // `Fwd`; its pending slot answers `Parked` rather than reading as Unbound.
    let exprs = working_all(
        &program,
        "NEWTYPE Foo = :{x :Number}\n\
         LET Fwd = Foo\n\
         PRINT Fwd",
    );
    let runtime = &mut test_run.runtime;
    runtime.enter_block(scope.id, exprs, scope);
    runtime
        .execute()
        .expect("dispatch with bare-name park should complete");
    let captured = buf.borrow().clone();
    // `Fwd` aliases `Foo`'s type identity; the exact rendering of that type
    // value is not what this test pins.
    assert!(
        !captured.is_empty(),
        "PRINT Fwd should produce output after the forward reference resolves",
    );
}

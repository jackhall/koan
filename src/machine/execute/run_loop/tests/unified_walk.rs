//! Cache-driven strict-only dispatch surface tests not covered elsewhere:
//! self-reference `LET Ty = Ty` (cache `Unbound`, wrap-slot terminalizes
//! without entering cycle detection) and bare-name forward reference to a
//! nominal-binder placeholder (cache `Parked`, splice walk installs combined
//! park, slot commits on wake).

use crate::builtins::test_support::TestRun;
use crate::machine::core::run_root_storage;
use crate::machine::KErrorKind;
use crate::parse::parse;

/// Self-reference `LET Ty = Ty`: the consumer sees its own placeholder as
/// hidden under index-gating (same idx, LET binders aren't nominal), so the
/// cache holds `Unbound("Ty")` and the wrap-slot terminal surfaces
/// `UnboundName`. Cycle detection only fires on visible Parked outcomes — a
/// separate path.
#[test]
fn self_referential_let_surfaces_unbound_name() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let exprs = parse("LET Ty = Ty").expect("parse should succeed");
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(scope.id, exprs, scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(ids[0]) {
        Err(e) => e.clone(),
        Ok(()) => panic!("self-referential LET should surface UnboundName"),
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
    let region = run_root_storage();
    let (mut test_run, buf) = TestRun::with_buf(&region);
    let scope = test_run.scope;
    // STRUCT (like MODULE) is a nominal binder, so the placeholder is visible
    // to the forward reference and parks rather than reading as Unbound.
    let exprs = parse(
        "NEWTYPE Foo = :{x :Number}\n\
         LET Fwd = Foo\n\
         PRINT Fwd",
    )
    .expect("parse should succeed");
    let runtime = &mut test_run.runtime;
    runtime.enter_block(scope.id, exprs, scope);
    runtime
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

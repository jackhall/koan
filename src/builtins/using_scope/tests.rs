//! `USING … SCOPE` block-scoped module opening.
//!
//! Module names carry a lowercase letter (`Mod`, `Res`) because the token
//! classifier reads all-uppercase names as keywords; dispatch keywords
//! (`DBL`, `GETIT`, `GETV`, `NOOP`) stay all-uppercase.

use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::KObject;
use crate::machine::{KErrorKind, KoanRegion};

#[test]
fn using_surfaces_module_value_as_bare_name() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "MODULE Mod = (LET val = 42)");
    let result = run_one(scope, parse_one("USING Mod SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

#[test]
fn using_surfaces_module_function_for_bare_dispatch() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Mod = (LET dbl = (FN (DBL x :Number) -> Number = (x)))",
    );
    let result = run_one(scope, parse_one("USING Mod SCOPE (DBL 21)"));
    assert!(matches!(result, KObject::Number(n) if *n == 21.0));
}

#[test]
fn using_block_bind_persists_at_call_site() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "MODULE Mod = (LET val = 1)");
    run(scope, "USING Mod SCOPE (LET local = 5)");
    let result = run_one(scope, parse_one("local"));
    assert!(matches!(result, KObject::Number(n) if *n == 5.0));
}

/// Without the guard in `bind_value`'s borrowed-window arm, the surfaced
/// member would silently shadow the bind.
#[test]
fn using_block_bind_colliding_with_member_errors() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "MODULE Mod = (LET x = 1)");
    let err = run_one_err(scope, parse_one("USING Mod SCOPE (LET x = 2)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("collides with a surfaced module member") && msg.contains("`x`")),
        "expected collision ShapeError naming `x`, got {err}",
    );
}

/// A module function resolves its own internals in its captured (module)
/// scope, not the call site — opening the module must not change that.
#[test]
fn using_module_function_resolves_its_own_internals() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Mod = ((LET secret = 99) \
                       (LET getit = (FN (GETIT) -> Number = (secret))))",
    );
    let result = run_one(scope, parse_one("USING Mod SCOPE (GETIT)"));
    assert!(matches!(result, KObject::Number(n) if *n == 99.0));
}

/// Multi-statement USING body runs as a block: a body-local `LET` reading a surfaced member
/// is visible to a later statement, and the *final* statement's value is the USING result
/// (not the first statement's). Pins block semantics through the transparent window.
#[test]
fn using_multi_statement_body_sequences_and_returns_last() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "MODULE Mod = (LET base = 7)");
    let result = run_one(
        scope,
        parse_one("USING Mod SCOPE ((LET local = base) (PRINT \"mid\") (local))"),
    );
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "expected the last statement's value (local = surfaced base = 7), got {:?}",
        result.ktype(),
    );
}

/// Window-first read order: the module's `val` wins over a same-name
/// call-site binding inside the block.
#[test]
fn using_window_shadows_call_site_binding() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "LET val = 1");
    run(scope, "MODULE Mod = (LET val = 7)");
    let result = run_one(scope, parse_one("USING Mod SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// SAFETY-anchor: closure escape for a functor-result module. `MAKE` returns
/// a module living in its per-call `CallFrame`; opening it with `USING` and
/// returning a closure that reads a surfaced member must keep both the
/// closure's transparent scope and the module's region alive past the block.
/// Run-root churn after the escape exercises drop discipline; under Miri this
/// pins the `Scope::child_transparent` / `alloc_scope` transmute sites
/// against use-after-free.
#[test]
fn using_functor_result_closure_escapes_soundly() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE) -> Module = (MODULE Res = (LET val = 7))\n\
         LET Inst = (MAKE)",
    );
    run(scope, "USING Inst SCOPE (FN (GETV) -> Number = (val))");
    // Churn the run-root region so a dangling reference into the dropped
    // USING/functor regions would surface under Miri.
    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..10 {
        run_one(scope, parse_one("NOOP"));
    }
    let result = run_one(scope, parse_one("GETV"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "GETV must still read the surfaced module `val` after escape + churn",
    );
}

/// SAFETY-anchor: `USING (MAKE) SCOPE …` opens an unbound module, so its
/// child-scope region's frame `Rc` lives only on the eager `m` arg, which
/// drops when the builtin body returns. The builtin roots that `Rc` in the
/// call-site region so the borrowed window stays valid for the deferred
/// sub-dispatch and any escaping closure. Without the rooting this is an
/// immediate use-after-free; under Miri this pins the rooting path.
#[test]
fn using_temporary_functor_result_is_sound() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "FN (MAKE) -> Module = (MODULE Res = (LET val = 9))");
    run(scope, "USING (MAKE) SCOPE (FN (GETW) -> Number = (val))");
    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..10 {
        run_one(scope, parse_one("NOOP"));
    }
    let result = run_one(scope, parse_one("GETW"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 9.0),
        "GETW must read the rooted temporary module's `val` after escape + churn",
    );
}

/// `USING` on a non-module value: strict admission rejects the Number
/// carrier against the `m :Module` slot, and with no other overload the walk
/// surfaces `DispatchFailed`.
#[test]
fn using_on_non_module_fails_dispatch() {
    let region = KoanRegion::new();
    let scope = run_root_silent(&region);
    run(scope, "LET n = 5");
    let mut sched = KoanRuntime::new();
    let root = sched.dispatch_in_scope(parse_one("USING n SCOPE (1)"), scope);
    sched
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = sched
        .read_result(root)
        .err()
        .expect("expected a DispatchFailed in the dispatch slot");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for USING on a Number, got {err}",
    );
}

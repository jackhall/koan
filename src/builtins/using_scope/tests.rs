//! `USING … SCOPE` block-scoped module opening.
//!
//! Covers the read window (bare value + bare op dispatch), bind forwarding/persistence,
//! the surfaced-member collision rejection, module-function internal-scope resolution,
//! and the functor-result closure-escape path (the last under Miri for the
//! `new_transparent` anchor unsafe sites).
//!
//! Module names carry a lowercase letter (`Mod`, `Res`) because the token classifier
//! rejects all-uppercase single/multi-letter names as type tokens (those read as
//! keywords); dispatch keywords (`DBL`, `GETIT`, `GETV`, `NOOP`) stay all-uppercase.

use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::execute::Scheduler;
use crate::machine::model::KObject;
use crate::machine::{KErrorKind, RuntimeArena};

/// Bare-name read of a module value member inside the block. `val` is surfaced from the
/// module's `data` window and resolves without the `Mod.` qualifier.
#[test]
fn using_surfaces_module_value_as_bare_name() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "MODULE Mod = (LET val = 42)");
    let result = run_one(scope, parse_one("USING Mod SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

/// Bare-op dispatch inside the block. A module member function bound via
/// `LET dbl = (FN (DBL …) …)` registers `DBL` in the module's `functions` bucket; the
/// window surfaces it so `DBL 21` dispatches without the qualifier.
#[test]
fn using_surfaces_module_function_for_bare_dispatch() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE Mod = (LET dbl = (FN (DBL x :Number) -> Number = (x)))",
    );
    let result = run_one(scope, parse_one("USING Mod SCOPE (DBL 21)"));
    assert!(matches!(result, KObject::Number(n) if *n == 21.0));
}

/// A bind made inside the block forwards to the call site and persists after the block
/// ends — `local` is readable at the call site once `USING` returns.
#[test]
fn using_block_bind_persists_at_call_site() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "MODULE Mod = (LET val = 1)");
    run(scope, "USING Mod SCOPE (LET local = 5)");
    let result = run_one(scope, parse_one("local"));
    assert!(matches!(result, KObject::Number(n) if *n == 5.0));
}

/// A block-local bind whose name collides with a surfaced module member is rejected with
/// a clean `ShapeError` — without the guard the window would silently shadow the bind.
#[test]
fn using_block_bind_colliding_with_member_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "MODULE Mod = (LET x = 1)");
    let err = run_one_err(scope, parse_one("USING Mod SCOPE (LET x = 2)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("collides with a surfaced module member") && msg.contains("`x`")),
        "expected collision ShapeError naming `x`, got {err}",
    );
}

/// A module function dispatched inside the block resolves its *own* internal names in the
/// module's lexical scope, not the call site: `GETIT`'s body reads the module-private
/// `secret`, which is in its captured (module) scope regardless of where it is called.
#[test]
fn using_module_function_resolves_its_own_internals() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "MODULE Mod = ((LET secret = 99) \
                       (LET getit = (FN (GETIT) -> Number = (secret))))",
    );
    let result = run_one(scope, parse_one("USING Mod SCOPE (GETIT)"));
    assert!(matches!(result, KObject::Number(n) if *n == 99.0));
}

/// Module names win over a same-name call-site binding inside the block (window-first
/// read order). `val` is 1 at the call site but 7 in the module; the block reads 7.
#[test]
fn using_window_shadows_call_site_binding() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET val = 1");
    run(scope, "MODULE Mod = (LET val = 7)");
    let result = run_one(scope, parse_one("USING Mod SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// Closure escape for a functor-result module. `MAKE` returns a module living in its
/// per-call `CallArena`; opening it with `USING` and returning a closure that reads a
/// surfaced member must keep both the closure's transparent scope and the module's arena
/// alive past the block. Run-root churn after the escape exercises drop discipline; under
/// Miri this pins the `CallArena::new_transparent` anchor unsafe sites against
/// use-after-free.
#[test]
fn using_functor_result_closure_escapes_soundly() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (MAKE) -> Module = (MODULE Res = (LET val = 7))\n\
         LET inst = (MAKE)",
    );
    // The block defines (and forwards-registers) `GETV`, whose body reads the module's
    // surfaced `val`. The closure captures the transparent scope and escapes the block.
    run(scope, "USING inst SCOPE (FN (GETV) -> Number = (val))");
    // Churn the run-root arena: allocate and dispatch unrelated work so a dangling
    // reference into the dropped USING/functor arenas would surface under Miri.
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

/// Temporary (unbound) functor-result module: `USING (MAKE) SCOPE …` opens a module never
/// bound to a name, so its child-scope arena's frame `Rc` lives *only* on the eager `m`
/// arg — which drops when the builtin body returns. The builtin roots that `Rc` in the
/// call-site arena, so the borrowed window stays valid both while the block runs (the
/// block is a deferred sub-dispatch, after the body has returned) and for a closure that
/// escapes it. Without the rooting this is an immediate use-after-free; under Miri this
/// pins the rooting path.
#[test]
fn using_temporary_functor_result_is_sound() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (MAKE) -> Module = (MODULE Res = (LET val = 9))");
    // Module opened inline, never bound. The block reads the surfaced `val` and returns a
    // closure that captures it.
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

/// `USING` on a non-module value: the eager-resolve path splices `Future(KObject::Number)`
/// into the `m :Module` wrap-slot and commits to the tentative pick from
/// `resolve_dispatch`. `bind` then rejects the Number-carrier against the Module-typed slot
/// as `TypeMismatch` (a per-slot terminal). Pre-eager-resolve the re-resolution walk
/// surfaced `DispatchFailed` out of `execute()`. Pins that a misuse degrades gracefully.
#[test]
fn using_on_non_module_fails_dispatch() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET n = 5");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("USING n SCOPE (1)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Ok(_) => panic!("USING on a non-module should fail dispatch"),
        Err(e) => e,
    };
    assert!(
        matches!(&err.kind, KErrorKind::TypeMismatch { .. }),
        "expected TypeMismatch for USING on a Number, got {err}",
    );
}

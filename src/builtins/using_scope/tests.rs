//! `USING … SCOPE` block-scoped module opening.
//!
//! Module names carry a lowercase letter (`Mod`, `Res`) because the token
//! classifier reads all-uppercase names as keywords; dispatch keywords
//! (`DBL`, `GETIT`, `GETV`, `NOOP`) stay all-uppercase.

use std::rc::Rc;

use crate::builtins::test_support::{
    delivered_with_host, parse_one, run, run_one, run_one_err, run_root_bare, run_root_silent,
};
use crate::machine::core::{run_root_storage, BindingIndex, Scope};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{Carried, KObject};
use crate::machine::KErrorKind;

#[test]
fn using_surfaces_module_value_as_bare_name() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "MODULE Mod = (LET val = 42)");
    let result = run_one(scope, parse_one("USING Mod SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

#[test]
fn using_surfaces_module_function_for_bare_dispatch() {
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET n = 5");
    let mut runtime = KoanRuntime::new();
    let root = runtime.dispatch_in_scope(parse_one("USING n SCOPE (1)"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = runtime
        .result_error(root)
        .expect_err("expected a DispatchFailed in the dispatch slot");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for USING on a Number, got {err}",
    );
}

/// SAFETY-anchor: the transparent-window transitive-root exception documented on `Witness for
/// Carrier` and `Scope::resident_witness`. A module binding's stored reach lives in the *module's*
/// own arena, not the reading window's call-site arena -- sound only because the window's overlay
/// fold (mirrored here, as `USING`'s own body performs it in `builtins/using_scope.rs`) mints the
/// module's own carrier into the call-site arena before any read, rooting the module region
/// transitively. Drops every other handle on both the module and foreign frames before reading the
/// carrier's reach back; under Miri this is a use-after-free the moment that rooting is missing.
#[test]
fn using_window_value_read_reach_survives_under_module_root() {
    let foreign_storage = run_root_storage();
    let foreign_weak = Rc::downgrade(&foreign_storage);

    let module_storage = run_root_storage();
    let module_weak = Rc::downgrade(&module_storage);
    let module_scope = run_root_bare(&module_storage);

    // Bind a value in the module scope whose stored reach names the foreign frame -- minted for
    // real into the module's own arena via `host_reach_of`, the same primitive `adopt_sealed` uses
    // to root a functor result's reach at module-bind time.
    let value_obj = module_scope.brand().alloc_object(KObject::Number(1.0));
    let cell = delivered_with_host(Carried::Object(value_obj), Rc::clone(&foreign_storage));
    let stored_reach = module_scope.host_reach_of(&cell);
    // Drop the envelope now: it must not be what keeps `foreign_storage` alive below — the
    // stored reach it minted into `module_scope`'s own arena is what the test exercises.
    drop(cell);
    module_scope
        .bind_value(
            "val".to_string(),
            value_obj,
            BindingIndex::value(0),
            stored_reach,
        )
        .expect("fresh binding name in an unborrowed scope");

    let call_site_storage = run_root_storage();
    let call_site_scope = run_root_bare(&call_site_storage);
    let window = call_site_scope
        .brand()
        .alloc_scope(Scope::child_transparent(
            call_site_scope,
            module_scope.bindings(),
        ));

    // Mirror `USING`'s own overlay fold (`builtins/using_scope.rs`): mint the opened module's own
    // carrier into the window's (call-site) arena at overlay construction, before any read through
    // the window -- the step that roots the module's region transitively.
    let window_root_dummy = window.brand().alloc_object(KObject::Number(0.0));
    let window_root_cell = delivered_with_host(
        Carried::Object(window_root_dummy),
        Rc::clone(&module_storage),
    );
    let _ = window.host_reach_of(&window_root_cell);
    drop(window_root_cell);

    let carrier = window
        .resolve_value_carrier("val", None)
        .expect("val is bound in the module scope, surfaced through the transparent window")
        .bound()
        .expect("val is fully bound, not a placeholder");

    drop(module_storage);
    drop(foreign_storage);
    assert!(
        module_weak.upgrade().is_some(),
        "the window's overlay fold roots the module region transitively"
    );
    assert!(
        foreign_weak.upgrade().is_some(),
        "the module's own arena, rooted transitively, keeps the entry's stored reach set -- and \
         the foreign frame it names -- alive"
    );

    let foreign_region_owner = foreign_weak.upgrade().unwrap();
    assert!(
        carrier
            .witness()
            .reach_covers(None, foreign_region_owner.region()),
        "the read carrier's reach still covers the foreign region after every other handle drops"
    );
}

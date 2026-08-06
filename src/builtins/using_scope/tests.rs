//! `USING … SCOPE` block-scoped module opening.
//!
//! Module names carry a lowercase letter (`some_module`, `res`) because the token
//! classifier reads all-uppercase names as keywords; dispatch keywords
//! (`DBL`, `GETIT`, `GETV`, `NOOP`) stay all-uppercase.
//!
//! - [`type_members`] — the window's type channel: a module's type members named in type
//!   positions inside the block, and the collision guard over them.

mod type_members;

use std::rc::Rc;

use crate::builtins::test_support::{parse_one, per_call_storage, run_root_bare, TestRun};
use crate::machine::model::{Carried, KObject};
use crate::machine::KErrorKind;
use crate::machine::{program_storage, run_root_storage, BindingIndex, FrameCoverage};

#[test]
fn using_surfaces_module_value_as_bare_name() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = (LET val = 42)");
    let result = test_run.run_one(parse_one(&program, "USING some_module SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0));
}

#[test]
fn using_surfaces_module_function_for_bare_dispatch() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = (LET dbl = (FN (DBL x :Number) -> Number = (x)))");
    let result = test_run.run_one(parse_one(&program, "USING some_module SCOPE (DBL 21)"));
    assert!(matches!(result, KObject::Number(n) if *n == 21.0));
}

#[test]
fn using_block_bind_persists_at_call_site() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = (LET val = 1)");
    test_run.run("USING some_module SCOPE (LET local = 5)");
    let result = test_run.run_one(parse_one(&program, "local"));
    assert!(matches!(result, KObject::Number(n) if *n == 5.0));
}

/// Without the guard the op-apply runs before forwarding a value write through a transparent
/// window, the surfaced member would silently shadow the bind.
#[test]
fn using_block_bind_colliding_with_member_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = (LET x = 1)");
    let err = test_run.run_one_err(parse_one(&program, "USING some_module SCOPE (LET x = 2)"));
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "MODULE some_module = ((LET secret = 99) \
                       (LET getit = (FN (GETIT) -> Number = (secret))))",
    );
    let result = test_run.run_one(parse_one(&program, "USING some_module SCOPE (GETIT)"));
    assert!(matches!(result, KObject::Number(n) if *n == 99.0));
}

/// Multi-statement USING body runs as a block: a body-local `LET` reading a surfaced member
/// is visible to a later statement, and the *final* statement's value is the USING result
/// (not the first statement's). Pins block semantics through the transparent window.
#[test]
fn using_multi_statement_body_sequences_and_returns_last() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("MODULE some_module = (LET base = 7)");
    let result = test_run.run_one(parse_one(
        &program,
        "USING some_module SCOPE ((LET local = base) (PRINT \"mid\") (local))",
    ));
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET val = 1");
    test_run.run("MODULE some_module = (LET val = 7)");
    let result = test_run.run_one(parse_one(&program, "USING some_module SCOPE (val)"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// `MODULE` and its `USING` in the **same block**, both inside one per-call frame: the module's
/// child scope and the window live in the same region, so the window door's fold is empty — the
/// library's self rule strips the destination region from what the envelope retains — and the
/// window's backing is held by the very frame it is built in. The co-located shape of the door's
/// one-operand fold, which the top-level tests above reach only through the run-root region.
#[test]
fn using_opens_a_module_declared_in_the_same_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (RUNIT) -> Number = ((MODULE some_module = (LET v = 3)) (USING some_module SCOPE (v)))",
    );
    let result = test_run.run_one(parse_one(&program, "RUNIT"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "a module declared in the calling block opens against that block's own region",
    );
}

/// The same block, but the module's region is **not** the window's: `MAKEIT`'s per-call frame hosts
/// the child scope, the bind adopts the module into `RUNIT`'s frame, and the `USING` window is built
/// there too — so the envelope's members, not the self rule, are what keep the surfaced bindings
/// alive, and both frames die at the call's end. The cross-region twin of the test above.
#[test]
fn using_opens_a_bound_functor_module_in_the_declaring_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (MAKEIT) -> Module = (MODULE res = (LET v = 9))");
    test_run.run("FN (RUNIT) -> Number = ((LET inst = (MAKEIT)) (USING inst SCOPE (v)))");
    let result = test_run.run_one(parse_one(&program, "RUNIT"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 9.0),
        "the window must read the functor module's member while both per-call frames are live",
    );
}

/// A window-surfaced member read *after* the `USING` statement, from a later statement of the same
/// block: the block's `LET k = v` forwards the surfaced value out of the window, so the read
/// outlives the window's own statement while staying inside the declaring frame.
#[test]
fn using_window_member_forwards_to_a_later_statement_of_the_same_block() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (MAKEIT) -> Module = (MODULE res = (LET v = 11))");
    test_run.run("FN (RUNIT) -> Number = ((USING (MAKEIT) SCOPE (LET k = v)) (k))");
    let result = test_run.run_one(parse_one(&program, "RUNIT"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 11.0),
        "a bind forwarded out of the window survives to the block's next statement",
    );
}

/// Closure escape for a functor-result module. `MAKE` returns a module living in its per-call
/// `CallFrame`; opening it with `USING` and returning a closure that reads a surfaced member must
/// keep both the closure's transparent scope and the module's region alive past the block. Run-root
/// churn after the escape exercises drop discipline.
#[test]
fn using_functor_result_closure_escapes_soundly() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (MAKE) -> Module = (MODULE res = (LET val = 7))\n\
         LET inst = (MAKE)",
    );
    test_run.run("USING inst SCOPE (FN (GETV) -> Number = (val))");
    // Churn the run-root region so a dangling reference into the dropped
    // USING/functor regions would surface under Miri.
    test_run.run("FN (NOOP) -> Number = (1)");
    for _ in 0..10 {
        test_run.run_one(parse_one(&program, "NOOP"));
    }
    let result = test_run.run_one(parse_one(&program, "GETV"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "GETV must still read the surfaced module `val` after escape + churn",
    );
}

/// `USING (MAKE) SCOPE …` opens an unbound module, so its child-scope region's frame `Rc` lives
/// only on the eager `m` arg, which drops when the builtin body returns. `Scope::open_module_window`
/// roots that `Rc` in the call-site region so the borrowed window stays valid for the deferred
/// sub-dispatch and any escaping closure.
///
/// Plain `cargo test`: the rooting is no longer a step the builtin can omit — the window door mints
/// the module's delivered coverage into the window's own region as it builds the window, and the
/// bare-window constructor is core-private — so what is left to check is the language-level result.
/// The library contract the rooting relies on (a description read under a *transitive* root) is
/// pinned under Miri by workgraph's `lift_reads_a_description_hosted_under_a_transitive_root`.
#[test]
fn using_temporary_functor_result_is_sound() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (MAKE) -> Module = (MODULE res = (LET val = 9))");
    test_run.run("USING (MAKE) SCOPE (FN (GETW) -> Number = (val))");
    test_run.run("FN (NOOP) -> Number = (1)");
    for _ in 0..10 {
        test_run.run_one(parse_one(&program, "NOOP"));
    }
    let result = test_run.run_one(parse_one(&program, "GETW"));
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET n = 5");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "USING n SCOPE (1)"),
        ),
        scope,
    );
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

/// A `USING` window's overlay is the one scope whose `region_owner` is the *call site* while the
/// bindings it surfaces live in the **module's** region — the single place residence and the reading
/// scope diverge. Residence rides the value's own description, not the reading scope, so a record
/// read through the window reports the module's region as its home. That is what makes the crossing
/// out of the window a *priceable* home crossing: `copy_or_pin`'s home-crossing test holds
/// and the chooser runs. A host taken from the reading scope would own no substrate here, and the
/// same crossing would short-circuit to Pin without ever being priced.
///
/// (Gated out of the forced-seam builds, which override the chooser wholesale.)
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[test]
fn using_window_value_prices_against_the_module_region_it_lives_in() {
    use crate::machine::core::{FoldingBrand, FrameStorageExt};
    use crate::machine::model::{copy_or_pin, Held, Record, RegionEscape, TypeRegistry};
    use crate::witnessed::FoldedPlacement;

    let module_storage = per_call_storage();
    let module_scope = run_root_bare(&module_storage);
    let types = TypeRegistry::new();

    // A plain-data record built in the module's own region: no leaf borrows home, so the chooser
    // has a real decision to make once the crossing is recognized as a home crossing at all.
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
        module_storage.brand().handle(),
    ))
    .with_holder(&owned_cells);
    let fields = Record::from_pairs(vec![("a".to_string(), Held::Object(KObject::Number(1.0)))]);
    let record = door.alloc_object_folded(KObject::record_of_held(door, fields, &types));
    let sealed =
        module_scope.seal_reaching(Carried::Object(record), module_scope.mint_born_here(false));
    module_scope
        .bind_value_direct(
            "rec".to_string(),
            sealed,
            BindingIndex::value(0),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("fresh binding name in an unborrowed scope");

    // The window: a transparent overlay whose `region_owner` is the call-site frame, not the
    // module's — the one place residence and the reading scope genuinely diverge.
    let call_site_storage = run_root_storage();
    let call_site_scope = run_root_bare(&call_site_storage);
    let window = call_site_scope.alloc_transparent_window_for_test(module_scope.bindings());
    // `USING`'s own overlay fold, which roots the module's arena under the call-site region.
    let _window_reach = window.mint_retained(&[&FrameCoverage::of(Rc::clone(&module_storage))]);

    let delivered = window
        .resolve_value_delivered("rec", None)
        .expect("rec is bound in the module scope, surfaced through the transparent window")
        .bound()
        .expect("rec is fully bound, not a placeholder");

    assert!(
        delivered
            .open_at()
            .with_home_region(|host| std::ptr::eq(host, module_storage.region())),
        "residence is the value's own record: it lives in the module's region, not the call \
         site's, even though the window reads at the call site"
    );
    // The seam verb's own body, spelled out: price the crossing against the host the carrier names.
    let opened = delivered.open_at();
    let verb = opened.with_home_region(|host| match opened.value() {
        Carried::Object(KObject::Record(substrate, _)) => copy_or_pin(substrate, host),
        _ => panic!("expected a Record carrier"),
    });
    assert!(
        !matches!(verb, RegionEscape::Pin),
        "the module region owns the substrate, so the crossing is priceable and the chooser \
         decides — a call-site host would own nothing and force Pin unconditionally"
    );
}

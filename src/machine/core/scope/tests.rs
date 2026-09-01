//! Unit coverage for the environment-copy **facts**: the per-scope monotone binding-copy-cost memo
//! a bind accumulates, the readiness gate a copy of a captured chain runs before it rebuilds
//! anything, and the callable escape chooser those two price. Every fact here is a stored read —
//! nothing walks a binding table, and nothing waits.

use std::rc::Rc;

use super::*;
use crate::builtins::test_support::{TestRun, value_name};
use crate::machine::core::arena::CallFrame;
use crate::machine::core::bindings::{BindingIndex, WriteGate};
use crate::machine::model::{KObject, RegionEscape, copy_or_pin_callable};
use crate::machine::model::{RunRegistries, object_copy_cost};
use crate::machine::{ProducerId, program_storage, run_root_storage};

/// Bind `name` to the number `value` in `scope`, through the construction-time value door.
fn bind_number<'a>(scope: &'a Scope<'a>, name: &str, value: f64, registries: &RunRegistries) {
    let object = KObject::Number(value);
    let sealed = scope
        .seal_pure_value(&object)
        .expect("a scalar seals resident");
    scope
        .bind_value_direct(
            value_name(name, registries),
            sealed,
            BindingIndex::value(0),
            registries,
            &mut WriteGate::for_test(),
        )
        .expect("a fresh name binds");
}

/// The memo starts at zero and grows by each bound value's own copy weight — the currency a
/// substrate prices its cells in — with no walk over the table.
#[test]
fn the_binding_copy_cost_memo_accumulates_per_bind() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let scope = test_run.scope.alloc_child_under();

    assert_eq!(scope.bindings().binding_copy_cost(), 0);

    let one = object_copy_cost(&KObject::Number(1.0));
    bind_number(scope, "a", 1.0, &registries);
    assert_eq!(scope.bindings().binding_copy_cost(), one);
    bind_number(scope, "b", 2.0, &registries);
    assert_eq!(
        scope.bindings().binding_copy_cost(),
        one.saturating_add(one),
        "the memo is a monotone sum, one term per bind",
    );
}

/// A string's bytes are re-bumped by a copy, so its weight is the flat cell plus its length — the
/// memo carries the same asymmetry the cell-level pricing does.
#[test]
fn the_memo_prices_a_string_by_its_bytes() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let scope = test_run.scope.alloc_child_under();

    let text = scope.brand().allocator().text("abcdefgh");
    let object = KObject::KString(text);
    let sealed = scope.seal_pure_value(&object).expect("a string seals");
    scope
        .bind_value_direct(
            value_name("s", &registries),
            sealed,
            BindingIndex::value(0),
            &registries,
            &mut WriteGate::for_test(),
        )
        .expect("a fresh name binds");

    assert_eq!(
        scope.bindings().binding_copy_cost(),
        object_copy_cost(&object),
        "a string's weight includes its bytes",
    );
}

/// An open scope is not ready: its defining block has not finished, so a further bind is still
/// legal and a copy of it could miss one. Closing it is what makes the table final.
#[test]
fn an_open_scope_is_not_copy_ready() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope.alloc_child_under();

    assert!(!scope.is_copy_ready(), "an unclosed scope is not ready");
    scope.close();
    assert!(scope.is_copy_ready(), "a closed anonymous scope is ready");
}

/// A standing claim is an unfinalized binding — the exact condition the roadmap downgrades on — so
/// a closed scope carrying one is still not ready. The copy pins; nothing waits on the producer.
#[test]
fn a_claimed_scope_is_not_copy_ready() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let scope = test_run.scope.alloc_child_under();
    scope.close();

    scope
        .bindings()
        .install_placeholder(
            crate::machine::model::BinderSymbol::Value(value_name("pending", &registries)),
            ProducerId::for_test(3),
            BindingIndex::value(1),
            &registries,
            &mut WriteGate::for_test(),
        )
        .expect("a fresh claim installs");

    assert!(
        !scope.is_copy_ready(),
        "a scope with an in-flight binder is not ready",
    );
}

/// A `USING … SCOPE` window borrows another scope's tables, so it has nothing of its own to
/// rebuild: not ready, whatever its closed state.
#[test]
fn a_using_window_scope_is_not_copy_ready() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let opened = test_run.scope.alloc_child_under();
    opened.close();
    let window = test_run
        .scope
        .alloc_transparent_window_for_test(opened.bindings());
    window.close();

    assert!(
        !window.is_copy_ready(),
        "a borrowed-bindings window is not ready",
    );
}

/// The v1 engine models the block and frame scopes a closure chain is made of. A `SIG` decl scope
/// carries a live slot collector and a `MODULE` body an announced window and group record, neither
/// of which it rebuilds — so both answer not-ready rather than being copied wrong.
#[test]
fn a_sig_or_module_kinded_scope_is_not_copy_ready() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();

    let sig = test_run
        .scope
        .alloc_child_under_sig(crate::builtins::test_support::type_name(
            "Shape",
            &registries,
        ));
    sig.close();
    assert!(!sig.is_copy_ready(), "a SIG decl scope is not ready");

    let module = test_run.scope.alloc_child_under_module(None);
    module.close();
    assert!(!module.is_copy_ready(), "a MODULE body is not ready");
}

/// The per-call walk stops at the innermost eternal home: those scopes are referenced verbatim by a
/// copy, so they are neither counted nor gated. A chain rooted at the run scope — itself eternal —
/// contributes only the per-call frame scopes below it.
#[test]
fn the_per_call_chain_stops_at_the_eternal_home() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);

    let eternal = test_run.scope.alloc_child_under();
    assert_eq!(
        eternal.per_call_chain().count(),
        0,
        "an eternal-homed scope has no per-call portion of its own",
    );

    let frame: Rc<CallFrame> = CallFrame::new(eternal);
    frame.with_scope(|inner| {
        let block = inner.alloc_child_under();
        assert_eq!(
            block.per_call_chain().count(),
            2,
            "the block and its frame scope are per-call; the eternal parent is not",
        );
    });
}

/// The chain cost sums each per-call scope's memo and stops at the eternal split — a value bound in
/// an eternal-homed ancestor is not part of what a consolidation would rebuild.
#[test]
fn the_chain_cost_sums_only_the_per_call_portion() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();

    let eternal = test_run.scope.alloc_child_under();
    bind_number(eternal, "outer", 1.0, &registries);

    let frame: Rc<CallFrame> = CallFrame::new(eternal);
    frame.with_scope(|inner| {
        bind_number(inner, "inner", 2.0, &registries);
        assert_eq!(
            inner.chain_copy_cost(),
            object_copy_cost(&KObject::Number(2.0)),
            "the eternal ancestor's own bindings are not in the chain's cost",
        );
    });
}

/// The chooser pins a **foreign** crossing outright: the innermost captured region is not the
/// crossing's host, which mirrors the substrate rule. Pricing a consolidation out of an
/// intermediate host is callable-copy-tuning's.
// A forced verification build overrides the chooser's table outright, so only the
// cost-driven build can assert what it decides.
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[test]
fn the_callable_chooser_pins_a_foreign_crossing() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);

    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);
    frame.with_scope(|inner| {
        inner.close();
        assert_eq!(
            copy_or_pin_callable(inner, test_run.scope.region()),
            RegionEscape::Pin,
            "a captured chain homed elsewhere than the host pins",
        );
    });
}

/// An unready chain pins whatever it would cost: readiness is asked before price, and the answer is
/// a pin rather than a wait.
// A forced verification build overrides the chooser's table outright, so only the
// cost-driven build can assert what it decides.
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[test]
fn the_callable_chooser_pins_an_unready_chain() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);

    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);
    frame.with_scope(|inner| {
        let host = inner.region();
        assert_eq!(
            copy_or_pin_callable(inner, host),
            RegionEscape::Pin,
            "an unclosed captured scope pins",
        );
        inner.close();
        assert_eq!(
            copy_or_pin_callable(inner, host),
            RegionEscape::Consolidate,
            "the same chain, closed, is a priceable home crossing",
        );
    });
}

/// The chooser is genuinely **priced**, not a constant: an environment grown large against the
/// region a pin would retain reaches `Pin`. How large that is depends on the bump's chunk growth,
/// so the test searches for the crossing rather than assuming where it sits — what it pins is that
/// a crossing exists at all, which is what makes the cheap-environment verdict a decision.
// A forced verification build overrides the chooser's table outright, so only the
// cost-driven build can assert what it decides.
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[test]
fn the_callable_chooser_pins_a_large_enough_environment() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();

    // A fresh frame per size: a bind into a closed scope is illegal, so the environment has to be
    // built before the scope closes and the chooser can be asked.
    let pinned_at = (1usize..40).find(|entries| {
        let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);
        frame.with_scope(|inner| {
            for entry in 0..*entries {
                let text = inner.brand().allocator().text(&"x".repeat(512));
                let object = KObject::KString(text);
                let sealed = inner.seal_pure_value(&object).expect("a string seals");
                inner
                    .bind_value_direct(
                        value_name(&format!("s{entry}"), &registries),
                        sealed,
                        BindingIndex::value(0),
                        &registries,
                        &mut WriteGate::for_test(),
                    )
                    .expect("a fresh name binds");
            }
            inner.close();
            copy_or_pin_callable(inner, inner.region()) == RegionEscape::Pin
        })
    });

    assert!(
        pinned_at.is_some(),
        "a growing captured environment must reach the pin verdict",
    );
}

/// A scalar bind leaves the environment cheap against the region's allocated total, so the same
/// ready home crossing consolidates.
// A forced verification build overrides the chooser's table outright, so only the
// cost-driven build can assert what it decides.
#[cfg(not(any(feature = "seam-force-copy", feature = "seam-force-pin")))]
#[test]
fn the_callable_chooser_consolidates_a_cheap_environment() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();

    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);
    frame.with_scope(|inner| {
        // Bulk allocated into the region without being bound: the pin would retain all of it, and
        // the environment's own weight is one scalar.
        let _bulk = inner.brand().allocator().text(&"x".repeat(4096));
        bind_number(inner, "n", 1.0, &registries);
        inner.close();

        assert_eq!(
            copy_or_pin_callable(inner, inner.region()),
            RegionEscape::Consolidate,
            "a small environment against a large pinned total consolidates",
        );
    });
}

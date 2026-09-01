//! Unit coverage for the environment copy's **structure** — what the rebuilt chain is wired to,
//! which the region counters the acceptance suite reads cannot see. A count says how many regions
//! survive; it cannot say whether two copied closures point at one copied scope or at two, nor
//! whether a copied binding points back at the source it claims to have left.
//!
//! The destination is the source's own region here. That is deliberate: the cross-region *release*
//! is a claim about reach, already stated where reach is observable (the `close_over` acceptance
//! suite, and the Miri slate for the borrow it rests on). What is left over is the wiring, and the
//! wiring is the same whichever region the copy lands in — the engine reads its destination out of
//! the fold brand it is handed either way.

use std::rc::Rc;

use super::*;
use crate::builtins::test_support::{TestRun, value_name};
use crate::machine::core::arena::CallFrame;
use crate::machine::core::bindings::{BindingIndex, WriteGate};
use crate::machine::core::kfunction::Body;
use crate::machine::core::tests::{body_no_op, unit_signature};
use crate::machine::model::RunRegistries;
use crate::machine::{program_storage, run_root_storage};
use crate::witnessed::FoldedPlacement;

/// Bind `name` in `scope` to a fresh closure capturing `scope` itself, and hand the callable back.
/// The shape every case here is built from: a binding whose value's captured scope is the very
/// scope holding it — the `scope → function → scope` ring the memo has to close.
fn bind_self_capturing<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    index: usize,
    registries: &RunRegistries,
) -> &'a KFunction<'a> {
    let cell = KFunction::alloc_captured_draft(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        registries,
    );
    let sealed = scope.store_function_cell(&cell);
    scope
        .bind_value_direct(
            value_name(name, registries),
            sealed,
            BindingIndex::value(index),
            registries,
            &mut WriteGate::for_test(),
        )
        .expect("a fresh name binds");
    let resident = cell.rest_into(scope.brand().handle());
    scope.open_function(&resident).value()
}

/// The address of the scope the callable bound under `name` captured.
///
/// An **address**, not a reference, for the reason the engine itself keys its memo by one: a sealed
/// binding re-anchors only at the borrow that opens it, so a reference read out here could not
/// outlive the read. Identity is all these assertions need, and an address carries it.
fn bound_captured_address(scope: &Scope<'_>, name: &str, registries: &RunRegistries) -> usize {
    let sealed = scope
        .bindings()
        .lookup_value(value_name(name, registries), None)
        .and_then(|hit| hit.bound())
        .unwrap_or_else(|| panic!("the copied scope binds `{name}`"));
    match sealed.open_at().value() {
        Carried::Object(KObject::KFunction(function)) => scope_address(function.captured_scope()),
        _ => panic!("`{name}` is bound to a callable"),
    }
}

/// A fold brand over `scope`'s own region — the door the engine is entered through.
fn door_over<'a>(scope: &'a Scope<'a>) -> FoldingBrand<'a> {
    FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(scope.brand().handle()))
}

/// Consolidate `top`, asserting the engine took it.
fn consolidated<'a>(top: &'a KFunction<'a>, door: FoldingBrand<'a>) -> &'a KFunction<'a> {
    match consolidate_object(&KObject::KFunction(top), door) {
        Some(KObject::KFunction(copy)) => copy,
        _ => panic!("a closed, data-only chain consolidates"),
    }
}

/// **A closure bound in the scope it captures copies to one whose captured scope binds the copy.**
/// The engine memoizes a source scope before filling its tables, so the binding's own rebuild
/// attaches under the copy already under construction rather than recursing forever or reaching
/// back into the source. The cycle half of the roadmap's second criterion, read off the wiring
/// instead of off a region count.
#[test]
fn a_self_capturing_binding_copies_to_one_that_binds_the_copy() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let top = bind_self_capturing(source, "f", 0, &registries);
        source.close();

        let copy = consolidated(top, door_over(source));
        let copied_scope = copy.captured_scope();
        assert!(
            !std::ptr::eq(copied_scope, source),
            "the copy captures a rebuilt scope, not the source",
        );
        assert_eq!(
            bound_captured_address(copied_scope, "f", &registries),
            scope_address(copied_scope),
            "the copied scope's own binding captures that same copied scope",
        );
    });
}

/// **Two closures over one defining scope copy to two closures over one copied scope.** The sharing
/// half of the same criterion, and the half a region count provably cannot discriminate: one copied
/// scope and two copied scopes retain identically. Here the two copies' captured scopes are
/// compared by address.
#[test]
fn sibling_bindings_copy_to_closures_over_one_copied_scope() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        bind_self_capturing(source, "a", 0, &registries);
        bind_self_capturing(source, "b", 1, &registries);
        let top = bind_self_capturing(source, "top", 2, &registries);
        source.close();

        let copied_scope = consolidated(top, door_over(source)).captured_scope();
        let copied_a = bound_captured_address(copied_scope, "a", &registries);
        let copied_b = bound_captured_address(copied_scope, "b", &registries);
        assert_eq!(
            copied_a, copied_b,
            "sibling closures over one defining scope share one copied scope",
        );
        assert_eq!(
            copied_a,
            scope_address(copied_scope),
            "and it is the scope the copied product itself captured",
        );
        assert_ne!(
            copied_a,
            scope_address(source),
            "which is not the source scope",
        );
    });
}

/// **An eternal-homed scope is referenced verbatim.** The copy stops at the eternal split — a
/// region that outlives everything which could retain it needs no rebuild — so the outermost copied
/// link's lexical parent is the source chain's own eternal home, by address. Rebuilding it instead
/// would mint a fresh run root per escape and lose every builtin the chain reaches through it.
#[test]
fn the_eternal_home_is_referenced_verbatim() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let top = bind_self_capturing(source, "f", 0, &registries);
        source.close();
        let eternal = source.innermost_eternal_home();

        let copied_scope = consolidated(top, door_over(source)).captured_scope();
        assert!(
            std::ptr::eq(
                copied_scope.outer().expect("a copied link has a parent"),
                eternal,
            ),
            "the outermost copied link takes the source chain's eternal home as its outer",
        );
    });
}

/// **An unready chain is declined, not waited on.** The engine re-checks readiness itself rather
/// than trusting the chooser's earlier verdict, so an open source scope answers `None` and the
/// caller rides the value verbatim. `None` is the only failure this module has — there is nothing
/// here to park on.
#[test]
fn an_open_source_chain_declines() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let top = bind_self_capturing(source, "f", 0, &registries);
        // Deliberately left open: a further bind is still legal, so a copy could miss one.
        assert!(
            consolidate_object(&KObject::KFunction(top), door_over(source)).is_none(),
            "an unclosed captured chain declines rather than copying a table that can still grow",
        );
    });
}

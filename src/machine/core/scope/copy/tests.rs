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
use crate::builtins::test_support::{operator_run, probe_symbol};
use crate::machine::core::arena::CallFrame;
use crate::machine::core::bindings::OperatorEntry;
use crate::machine::core::bindings::{BindingIndex, WriteGate};
use crate::machine::core::carrier_witness::GroupSeal;
use crate::machine::core::kfunction::Body;
use crate::machine::core::tests::{body_no_op, unit_signature};
use crate::machine::model::{KeywordSymbol, OperatorGroup, ReductionMode, RunRegistries};
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

/// Register a two-member `FoldLeft` group in `scope` under all three of its powerset probes — the
/// shape a `GROUP` declaration installs, and the shape a `CLOSE OVER` flatten copies out of an
/// enclosing chain. Hands back the source record's address and the probe keys.
fn register_powerset<'a>(
    scope: &'a Scope<'a>,
    registries: &RunRegistries,
) -> (usize, Vec<KeywordSymbol>) {
    let record = scope.birth_operator_group(
        &[probe_symbol("⊕"), probe_symbol("⊗")],
        ReductionMode::FoldLeft,
    );
    let seal = GroupSeal::of_delivered(scope, &record);
    let probes = vec![
        operator_run(&["⊕"], registries),
        operator_run(&["⊗"], registries),
        operator_run(&["⊕", "⊗"], registries),
    ];
    for probe in &probes {
        scope
            .register_operator_group_direct(
                *probe,
                seal.clone(),
                BindingIndex::value(0),
                registries,
                &mut WriteGate::for_test(),
            )
            .expect("a fresh powerset key registers");
    }
    (seal.address, probes)
}

/// **An operator registration no longer pins, and its table copies.** The readiness gate stopped
/// naming operators, so a scope holding a `GROUP`'s powerset consolidates; the copied table carries
/// the same probe set, and all three subset entries seal **one** reborn record — the per-record
/// memo doing its job, where a per-entry rebuild would have minted three where the source has one.
/// The reborn address differs from the source's by construction: that is the copy's whole point.
#[test]
fn an_operator_registration_copies_over_one_reborn_record() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let (source_address, probes) = register_powerset(source, &registries);
        let top = bind_self_capturing(source, "f", 0, &registries);
        source.close();

        let copied_scope = consolidated(top, door_over(source)).captured_scope();
        let copied = copied_scope.bindings().operator_entry_addresses();
        assert_eq!(
            copied.len(),
            probes.len(),
            "the copy holds one entry per source probe",
        );
        for probe in &probes {
            assert!(
                copied.iter().any(|(key, _)| key == probe),
                "every source probe key is registered in the copy",
            );
        }
        let reborn = copied[0].1;
        assert!(
            copied.iter().all(|(_, address)| *address == reborn),
            "every powerset entry seals one reborn record, as the source's entries share one",
        );
        assert_ne!(
            reborn, source_address,
            "and it is a fresh record, not the region-resident one the source sealed",
        );
    });
}

/// **The copy preserves the upsert's decisions.** A registration that was a silent no-op against
/// the source table is one against the copy, and a chaining-mode conflict is still a conflict.
/// The address arm cannot carry that: a reborn record has a fresh address by construction. What
/// carries it is the structural arm — `OperatorGroup::alloc` re-sorts and re-dedups, so the reborn
/// record renders a byte-identical `declaration_key`, which the copied entry stores verbatim.
#[test]
fn a_copied_table_preserves_the_upsert_decisions() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let (_, probes) = register_powerset(source, &registries);
        let top = bind_self_capturing(source, "f", 0, &registries);
        source.close();

        let copied_scope = consolidated(top, door_over(source)).captured_scope();
        let members = [probe_symbol("⊕"), probe_symbol("⊗")];

        let twin = copied_scope.birth_operator_group(&members, ReductionMode::FoldLeft);
        assert!(
            copied_scope
                .register_operator_group_direct(
                    probes[0],
                    GroupSeal::of_delivered(copied_scope, &twin),
                    BindingIndex::value(0),
                    &registries,
                    &mut WriteGate::for_test(),
                )
                .is_ok(),
            "an equal declaration against the copy is the silent no-op it was against the source",
        );

        let clashing = copied_scope.birth_operator_group(&members, ReductionMode::FoldRight);
        assert!(
            copied_scope
                .register_operator_group_direct(
                    probes[1],
                    GroupSeal::of_delivered(copied_scope, &clashing),
                    BindingIndex::value(0),
                    &registries,
                    &mut WriteGate::for_test(),
                )
                .is_err(),
            "and a disagreeing chaining mode is still the conflict it was against the source",
        );
    });
}

/// **An operator table prices its own rebuild.** The chooser reads `binding_copy_cost` to decide
/// whether a crossing is worth copying, so a table the engine now rebuilds has to enter that memo —
/// otherwise the gate's removal buys a copy the chooser priced at zero. A powerset charges its
/// record once and its entries once each, and the copied scope arrives at the same figure.
#[test]
fn an_operator_table_prices_its_rebuild() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let registries = RunRegistries::new();
    let frame: Rc<CallFrame> = CallFrame::new(test_run.scope);

    frame.with_scope(|source| {
        let before = source.bindings().binding_copy_cost();
        let (_, probes) = register_powerset(source, &registries);
        let charged = source.bindings().binding_copy_cost() - before;

        let record_bytes =
            (size_of::<OperatorGroup<'static>>() + 2 * size_of::<KeywordSymbol>()) as u64;
        let declaration = OperatorGroup::alloc(
            source.brand(),
            &[probe_symbol("⊕"), probe_symbol("⊗")],
            ReductionMode::FoldLeft,
        )
        .declaration_key();
        let entries =
            probes.len() as u64 * (size_of::<OperatorEntry<'static>>() + declaration.len()) as u64;
        assert_eq!(
            charged,
            record_bytes + entries,
            "the powerset charges its one record once and each subset entry its own bytes",
        );

        let top = bind_self_capturing(source, "f", 0, &registries);
        source.close();
        let with_operators = copied_cost(consolidated(top, door_over(source)));
        assert_eq!(
            with_operators - copied_cost_without_operators(test_run.scope, &registries),
            charged,
            "and the copy prices its own re-consolidation on the same terms",
        );
    });
}

/// The `binding_copy_cost` a copied callable's captured scope carries.
fn copied_cost(copy: &KFunction<'_>) -> u64 {
    copy.captured_scope().bindings().binding_copy_cost()
}

/// The same consolidation with **no** operator registration — the baseline the operator-table
/// charge is read against, so the measurement isolates the table from whatever the rebuilt callable
/// binding itself contributes.
fn copied_cost_without_operators<'a>(outer: &'a Scope<'a>, registries: &RunRegistries) -> u64 {
    let frame: Rc<CallFrame> = CallFrame::new(outer);
    frame.with_scope(|source| {
        let top = bind_self_capturing(source, "f", 0, registries);
        source.close();
        copied_cost(consolidated(top, door_over(source)))
    })
}

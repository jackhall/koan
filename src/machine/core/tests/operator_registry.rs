//! Tests for the per-scope operator registry —
//! [`crate::machine::core::Bindings::lookup_operator_group`] and
//! [`crate::machine::core::Scope::resolve_operator_group_delivered`]. The registry
//! parallels the function/type lookup layers: innermost visible registration wins,
//! a cross-group or undeclared probe misses. Unlike the type and function layers the walk
//! is innermost-*all-the-way*: the builtin groups seeded into the run-global root are
//! found last, so a declaring scope overrides them.
//!
//! A registry entry is a sealed carrier over a region-hosted record, so every assertion that reads
//! a resolved group does so inside the delivery envelope's own open — the record's borrow never
//! leaves the region that hosts it.

use std::rc::Rc;

use crate::builtins::test_support::{TestRun, operator_run, probe_symbol, run_root_bare};
use crate::machine::DeliveredOperatorGroup;
use crate::machine::core::{
    BindingIndex, CallFrame, GroupSeal, Scope, program_storage, run_root_storage,
};
use crate::machine::model::{OperatorGroup, ReductionMode, RunRegistries};

/// The declaration door a fixture takes: birth the record in `scope`'s own region and rest that
/// envelope, which is what every registry entry for this declaration then holds a bit-copy of.
fn declare<'a>(scope: &'a Scope<'a>, members: &[&str], mode: ReductionMode) -> GroupSeal<'a> {
    let members: Vec<_> = members.iter().map(|glyph| probe_symbol(glyph)).collect();
    GroupSeal::of_delivered(scope, &scope.birth_operator_group(&members, mode))
}

/// Arithmetic-shaped group: `+` and `-` fold left.
fn arithmetic_group<'a>(scope: &'a Scope<'a>) -> GroupSeal<'a> {
    declare(scope, &["+", "-"], ReductionMode::FoldLeft)
}

/// The address of the record a resolved envelope names — the identity every powerset key of one
/// declaration shares, read out as a number so nothing borrowed escapes the open.
fn record_address(delivered: &DeliveredOperatorGroup) -> usize {
    delivered.open(|group| group as *const OperatorGroup<'_> as usize)
}

#[test]
fn register_then_resolve_group_by_probe() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = arithmetic_group(scope);
    // A module registers the powerset; the key is the run digest a chain mixing both operators
    // computes.
    let key = operator_run(&["+", "-"], &registries);
    scope
        .register_operator_group_direct(
            key,
            group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let resolved = scope
        .resolve_operator_group_delivered(key, None)
        .expect("registered probe resolves");
    assert!(resolved.open(|group| {
        group.covers(&[probe_symbol("+")])
            && group.covers(&[probe_symbol("-")])
            && matches!(group.mode(), ReductionMode::FoldLeft)
    }));
}

#[test]
fn undeclared_probe_misses() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = arithmetic_group(scope);
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // `*` was never registered.
    assert!(
        scope
            .resolve_operator_group_delivered(operator_run(&["*"], &registries), None)
            .is_none()
    );
}

#[test]
fn cross_group_probe_misses() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = arithmetic_group(scope);
    // Only the within-group subsets are registered.
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            group.clone(),
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_operator_group_direct(
            operator_run(&["-"], &registries),
            group.clone(),
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_operator_group_direct(
            operator_run(&["+", "-"], &registries),
            group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // A chain mixing `+` with an operator from a different (unregistered) group
    // produces the probe "+ |", which nothing registered — a clean miss.
    assert!(
        scope
            .resolve_operator_group_delivered(operator_run(&["+", "|"], &registries), None)
            .is_none()
    );
}

#[test]
fn innermost_scope_shadows_outer() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let inner = outer.alloc_child_under();

    let outer_group = arithmetic_group(outer);
    let inner_group = declare(inner, &["+"], ReductionMode::FoldRight);

    outer
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            outer_group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    inner
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            inner_group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();

    // The inner registration wins the chain walk.
    let resolved = inner
        .resolve_operator_group_delivered(operator_run(&["+"], &registries), None)
        .expect("inner registration resolves");
    assert!(resolved.open(|group| matches!(group.mode(), ReductionMode::FoldRight)));

    // From the outer scope, only the outer registration is visible.
    let outer_resolved = outer
        .resolve_operator_group_delivered(operator_run(&["+"], &registries), None)
        .expect("outer registration resolves");
    assert!(outer_resolved.open(|group| matches!(group.mode(), ReductionMode::FoldLeft)));
}

#[test]
fn visibility_cutoff_hides_later_sibling_registration() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = arithmetic_group(scope);
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            group,
            BindingIndex::value(5),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // A consumer at cutoff 3 can't see a registration at index 5.
    assert!(
        scope
            .bindings()
            .lookup_operator_group(operator_run(&["+"], &registries), Some(3))
            .is_none()
    );
    // A consumer at cutoff 9 can.
    assert!(
        scope
            .bindings()
            .lookup_operator_group(operator_run(&["+"], &registries), Some(9))
            .is_some()
    );
}

#[test]
fn covers_gates_subset_membership() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = OperatorGroup::alloc(
        scope.brand(),
        &[probe_symbol("+"), probe_symbol("-")],
        ReductionMode::FoldLeft,
    );
    assert!(group.covers(&[probe_symbol("+"), probe_symbol("-")]));
    assert!(group.covers(&[probe_symbol("+")]));
    // `*` is not a member.
    assert!(!group.covers(&[probe_symbol("+"), probe_symbol("*")]));
}

/// The member slice is sorted and deduped at the allocation door, whatever order (and however many
/// repeats) the declaration hands in — the invariant `covers`' binary search reads.
#[test]
fn alloc_sorts_and_dedups_the_member_slice() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = OperatorGroup::alloc(
        scope.brand(),
        &[probe_symbol("-"), probe_symbol("+"), probe_symbol("-")],
        ReductionMode::FoldLeft,
    );
    let mut expected = vec![probe_symbol("+"), probe_symbol("-")];
    expected.sort_unstable();
    assert_eq!(group.member_symbols().collect::<Vec<_>>(), expected);
    assert!(group.covers(&[probe_symbol("-"), probe_symbol("+")]));
}

/// A scope may register a probe the builtins already claim (`+`): the walk is innermost-wins,
/// so that scope's chains reduce by its mode, while a chain written outside it still finds the
/// root's builtin additive group.
#[test]
fn inner_registration_of_a_builtin_probe_wins_inside_and_not_outside() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let inner = root.alloc_child_under();

    let group = declare(inner, &["+"], ReductionMode::FoldRight);
    inner
        .register_operator_group_direct(
            operator_run(&["+"], test_run.registries()),
            group,
            BindingIndex::value(1),
            test_run.registries(),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("a builtin probe is shadowable, not a rebind");

    let inside = inner
        .resolve_operator_group_delivered(operator_run(&["+"], test_run.registries()), None)
        .expect("the inner registration resolves");
    assert!(inside.open(|group| matches!(group.mode(), ReductionMode::FoldRight)));

    let outside = root
        .resolve_operator_group_delivered(operator_run(&["+"], test_run.registries()), None)
        .expect("the root's builtin additive group resolves");
    assert!(outside.open(|group| {
        matches!(group.mode(), ReductionMode::FoldLeft)
            && group.covers(&[probe_symbol("+"), probe_symbol("-")])
    }));
}

/// Upsert: re-registering a probe with an equal record is a no-op — the same address on the cheap
/// arm, and a **separately allocated** record of equal mode + member set on the structural arm, so
/// two `OP` statements over one symbol (two bucket overloads, one registry entry) do not collide.
#[test]
fn re_registering_an_equal_record_is_a_no_op() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);

    let first = declare(scope, &["+"], ReductionMode::FoldLeft);
    let second = declare(scope, &["+"], ReductionMode::FoldLeft);
    assert_ne!(
        record_address(&scope.lift_resident(first.sealed.duplicate())),
        record_address(&scope.lift_resident(second.sealed.duplicate())),
        "two declarations allocate two records, so the structural arm is what admits the second",
    );
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            first.clone(),
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            first,
            BindingIndex::value(2),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("an address-identical re-register is idempotent");
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            second,
            BindingIndex::value(3),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("an equal mode + member set is the same declaration");

    // The first registration's index stands, so the entry stays visible where it was declared.
    assert!(
        scope
            .bindings()
            .lookup_operator_group(operator_run(&["+"], &registries), Some(2))
            .is_some()
    );
}

/// Upsert: the same probe under a different chaining mode is a conflict — one scope declares one
/// mode per operator. The diagnostic names the probe.
#[test]
fn re_registering_a_conflicting_mode_errors() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);

    let fold = declare(scope, &["+"], ReductionMode::FoldLeft);
    let unary = declare(scope, &["+"], ReductionMode::Unary);
    scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            fold,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let error = scope
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            unary,
            BindingIndex::value(2),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect_err("a different chaining mode under one probe is a conflict");
    let message = error.to_string();
    assert!(
        message.contains('+') && message.contains("chaining mode"),
        "the mode-conflict diagnostic must name the probe; got: {message}"
    );
}

/// A `USING` window (a transparent scope borrowing a module's `Bindings`) surfaces the module's
/// operator registrations alongside its values: a chain written in the window resolves the
/// module's group.
#[test]
fn using_window_surfaces_the_modules_operator_group() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let root = run_root_bare(&region);

    let module = root.alloc_child_under_module(None);
    let group = declare(module, &["+"], ReductionMode::FoldRight);
    module
        .register_operator_group_direct(
            operator_run(&["+"], &registries),
            group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();

    // Outside the module the probe is undeclared.
    assert!(
        root.resolve_operator_group_delivered(operator_run(&["+"], &registries), None)
            .is_none()
    );

    // `USING vec_ops SCOPE (…)`: the window borrows the module's façade over the call site.
    let window = root.alloc_transparent_window_for_test(module.bindings());
    let resolved = window
        .resolve_operator_group_delivered(operator_run(&["+"], &registries), None)
        .expect("the window surfaces the module's registry entry");
    assert!(resolved.open(|group| matches!(group.mode(), ReductionMode::FoldRight)));
}

/// `register_group_under_all_subsets_direct` installs one entry per nonempty subset, all naming one
/// record, so any probe drawn from the member set resolves the same group — by **address**, which
/// is what one allocation behind the whole powerset buys.
#[test]
fn subset_registration_covers_every_probe_of_the_member_set() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = arithmetic_group(scope);
    scope
        .register_group_under_all_subsets_direct(
            &[probe_symbol("+"), probe_symbol("-")],
            group,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();

    let mut addresses = Vec::new();
    for probe in [&["+"][..], &["-"][..], &["+", "-"][..]] {
        let resolved = scope
            .resolve_operator_group_delivered(operator_run(probe, &registries), None)
            .unwrap_or_else(|| panic!("the probe `{probe:?}` must resolve the registered group"));
        addresses.push(record_address(&resolved));
    }
    assert!(
        addresses.windows(2).all(|pair| pair[0] == pair[1]),
        "every powerset key names the one record: {addresses:?}",
    );
    // The digest is order-insensitive and dedupes, so a repeated-operator run keys the same probe.
    assert_eq!(
        operator_run(&["-", "+", "-"], &registries),
        operator_run(&["+", "-"], &registries)
    );
}

/// The record dies with its declaring **region**, and nothing but a live carrier keeps it. A group
/// is declared into a per-call frame's own region, resolved one region down, and the declaring
/// frame's shell is dropped outright — the envelope's coverage (the `Weak → Rc` upgrade the lift
/// performed at the declaring scope) is the only thing left holding that region, so reading the
/// record afterwards depends on the lift retaining the declaring region rather than the reader's.
/// With the envelope gone the region frees whole, bump-hosted record included: there is no refcount
/// that could outlive it, which is what makes a dead scope's group unreachable rather than stale.
///
/// The memory shape — a bump-hosted `Copy` pointee with an empty member set, whose whole liveness
/// is the lift's upgrade at its hosting region — is pinned under Miri by the workgraph slate's
/// `lift_of_a_bump_hosted_value_with_no_members_outlives_its_declaring_handle`; this test drives it
/// through koan's registry under plain `cargo test`.
#[test]
fn resolved_group_survives_the_declaring_frames_shell_drop() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;

    let declaring: Rc<CallFrame> = CallFrame::new(root);
    let envelope: DeliveredOperatorGroup = declaring.with_scope(|scope| {
        let group = declare(scope, &["≺"], ReductionMode::FoldRight);
        scope
            .register_operator_group_direct(
                operator_run(&["≺"], test_run.registries()),
                group,
                BindingIndex::value(0),
                test_run.registries(),
                &mut crate::machine::WriteGate::for_test(),
            )
            .expect("the declaring scope owns the probe");
        // The reading chain sits one region further down, so the hit is an ancestor's and the lift
        // happens at the declaring scope.
        let reader: Rc<CallFrame> = CallFrame::new(scope);
        reader.with_scope(|chain_scope| {
            chain_scope
                .resolve_operator_group_delivered(operator_run(&["≺"], test_run.registries()), None)
                .expect("the declaring frame's registration resolves one region down")
        })
    });

    drop(declaring);

    assert!(
        envelope.open(|group| {
            group.covers(&[probe_symbol("≺")]) && matches!(group.mode(), ReductionMode::FoldRight)
        }),
        "the envelope's coverage keeps the declaring region alive across its frame's drop",
    );
}

/// The reach a resolved carrier records is the **declaring** scope's region, not the reader's: a
/// chain in a per-call frame resolves an ancestor's group through an envelope whose host is the
/// ancestor's region, so the record stays covered for as long as the envelope lives.
#[test]
fn resolved_carrier_reaches_the_declaring_ancestors_region() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let ancestor = test_run.scope;

    let group = declare(ancestor, &["~"], ReductionMode::FoldLeft);
    ancestor
        .register_operator_group_direct(
            operator_run(&["~"], test_run.registries()),
            group,
            BindingIndex::value(1),
            test_run.registries(),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();

    // A per-call frame opens its own region; the group was declared one region up.
    let frame: Rc<CallFrame> = CallFrame::new(ancestor);
    frame.with_scope(|inner| {
        assert!(
            !std::ptr::eq(inner.region(), ancestor.region()),
            "the frame child must open its own region for the assertion to say anything",
        );
        let resolved = inner
            .resolve_operator_group_delivered(operator_run(&["~"], test_run.registries()), None)
            .expect("the ancestor's registration resolves from the frame child");
        assert!(
            resolved
                .open_at()
                .with_home_region(|home| std::ptr::eq(home, ancestor.region())),
            "the envelope's host is the region that hosts the record",
        );
    });
}

/// The group context an `OP` declaration reads: a `GROUP` body answers with its own record even
/// though it is stamped `Module` (a group is a module), anonymous frames inside it are
/// transparent, and a plain module nested in the group body short-circuits to `None`.
#[test]
fn nearest_group_context_stops_at_a_plain_module() {
    let region = run_root_storage();
    let root = run_root_bare(&region);
    let group = OperatorGroup::alloc(
        root.brand(),
        &[probe_symbol("+"), probe_symbol("-")],
        ReductionMode::FoldLeft,
    );

    assert!(root.nearest_group_context().is_none());

    let group_scope = root.alloc_child_under_group(group, None);
    let in_group = group_scope
        .nearest_group_context()
        .expect("a GROUP body is its own group context");
    assert!(std::ptr::eq(in_group, group));

    // An anonymous frame inside the body (a block, a per-call scope) is transparent.
    let block = group_scope.alloc_child_under();
    assert!(
        block
            .nearest_group_context()
            .is_some_and(|g| std::ptr::eq(g, group))
    );

    // A plain module declared inside the group body is not a group.
    let nested_module = group_scope.alloc_child_under_module(None);
    assert!(nested_module.nearest_group_context().is_none());

    // Nor is a module that never carried a group.
    let plain_module = root.alloc_child_under_module(None);
    assert!(plain_module.nearest_group_context().is_none());
}

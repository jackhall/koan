//! Tests for the per-scope operator registry —
//! [`crate::machine::core::Bindings::lookup_operator_group`] and
//! [`crate::machine::core::Scope::resolve_operator_group_with_chain`]. The registry
//! parallels the function/type lookup layers: innermost visible registration wins,
//! a cross-group or undeclared probe misses. Unlike the type and function layers the walk
//! is innermost-*all-the-way*: the builtin groups seeded into the run-global root are
//! found last, so a declaring scope overrides them.

use std::collections::HashSet;

use crate::builtins::default_scope;
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::{run_root_storage, BindingIndex, FrameStorageExt, Scope};
use crate::machine::model::{probe_key, OperatorGroup, ReductionMode};

/// Arithmetic-shaped group: `+` and `-` fold left.
fn arithmetic_group() -> OperatorGroup {
    let members: HashSet<String> = ["+", "-"].iter().map(|s| s.to_string()).collect();
    OperatorGroup::new(members, ReductionMode::FoldLeft)
}

/// Single-member group over `sym`, in the given mode.
fn singleton_group(sym: &str, mode: ReductionMode) -> OperatorGroup {
    let members: HashSet<String> = [sym.to_string()].into_iter().collect();
    OperatorGroup::new(members, mode)
}

#[test]
fn register_then_resolve_group_by_probe() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());
    // A module registers the powerset; `probe_key` is the sorted-joined probe for a
    // chain mixing both operators — byte order sorts `+` before `-`.
    let key = probe_key(&["+", "-"]);
    assert_eq!(key, "+ -");
    scope
        .register_operator_group(key.clone(), group, BindingIndex::value(1))
        .unwrap();
    let resolved = scope
        .resolve_operator_group_with_chain(&key, None)
        .expect("registered probe resolves");
    assert!(resolved.covers(&["+"]));
    assert!(resolved.covers(&["-"]));
    assert_eq!(resolved.mode(), &ReductionMode::FoldLeft);
}

#[test]
fn undeclared_probe_misses() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());
    scope
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .unwrap();
    // `*` was never registered.
    assert!(scope.resolve_operator_group_with_chain("*", None).is_none());
}

#[test]
fn cross_group_probe_misses() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());
    // Only the within-group subsets are registered.
    scope
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .unwrap();
    scope
        .register_operator_group("-".to_string(), group, BindingIndex::value(1))
        .unwrap();
    scope
        .register_operator_group(probe_key(&["+", "-"]), group, BindingIndex::value(1))
        .unwrap();
    // A chain mixing `+` with an operator from a different (unregistered) group
    // produces the probe "+ |", which nothing registered — a clean miss.
    assert!(scope
        .resolve_operator_group_with_chain("+ |", None)
        .is_none());
}

#[test]
fn innermost_scope_shadows_outer() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let inner = region.brand().alloc_scope(outer.child_for_call());

    let outer_group = region.brand().alloc_operator_group(arithmetic_group());
    let inner_members: HashSet<String> = ["+"].iter().map(|s| s.to_string()).collect();
    let inner_group = region
        .brand()
        .alloc_operator_group(OperatorGroup::new(inner_members, ReductionMode::FoldRight));

    outer
        .register_operator_group("+".to_string(), outer_group, BindingIndex::value(1))
        .unwrap();
    inner
        .register_operator_group("+".to_string(), inner_group, BindingIndex::value(1))
        .unwrap();

    // The inner registration wins the chain walk.
    let resolved = inner
        .resolve_operator_group_with_chain("+", None)
        .expect("inner registration resolves");
    assert_eq!(resolved.mode(), &ReductionMode::FoldRight);

    // From the outer scope, only the outer registration is visible.
    let outer_resolved = outer
        .resolve_operator_group_with_chain("+", None)
        .expect("outer registration resolves");
    assert_eq!(outer_resolved.mode(), &ReductionMode::FoldLeft);
}

#[test]
fn visibility_cutoff_hides_later_sibling_registration() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());
    scope
        .register_operator_group("+".to_string(), group, BindingIndex::value(5))
        .unwrap();
    // A consumer at cutoff 3 can't see a registration at index 5.
    assert!(scope
        .bindings()
        .lookup_operator_group("+", Some(3))
        .is_none());
    // A consumer at cutoff 9 can.
    assert!(scope
        .bindings()
        .lookup_operator_group("+", Some(9))
        .is_some());
}

#[test]
fn covers_gates_subset_membership() {
    let group = arithmetic_group();
    assert!(group.covers(&["+", "-"]));
    assert!(group.covers(&["+"]));
    // `*` is not a member.
    assert!(!group.covers(&["+", "*"]));
}

/// A scope may register a probe the builtins already claim (`+`): the walk is innermost-wins,
/// so that scope's chains reduce by its mode, while a chain written outside it still finds the
/// root's builtin additive group.
#[test]
fn inner_registration_of_a_builtin_probe_wins_inside_and_not_outside() {
    let region = run_root_storage();
    let root = default_scope(&region, Box::new(std::io::sink()));
    let inner = region.brand().alloc_scope(root.child_for_call());

    let group = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::FoldRight));
    inner
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .expect("a builtin probe is shadowable, not a rebind");

    let inside = inner
        .resolve_operator_group_with_chain("+", None)
        .expect("the inner registration resolves");
    assert_eq!(inside.mode(), &ReductionMode::FoldRight);

    let outside = root
        .resolve_operator_group_with_chain("+", None)
        .expect("the root's builtin additive group resolves");
    assert_eq!(outside.mode(), &ReductionMode::FoldLeft);
    assert!(
        outside.covers(&["+", "-"]),
        "outside the declaring scope the builtin additive group stands"
    );
}

/// Upsert: re-registering a probe with an equal record — a distinct allocation carrying the same
/// mode and member set — is a no-op, so two `OP` statements over one symbol (two bucket overloads,
/// one registry entry) do not collide.
#[test]
fn re_registering_an_equal_record_is_a_no_op() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);

    let first = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::FoldLeft));
    let second = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::FoldLeft));
    scope
        .register_operator_group("+".to_string(), first, BindingIndex::value(1))
        .unwrap();
    scope
        .register_operator_group("+".to_string(), first, BindingIndex::value(2))
        .expect("a pointer-equal re-register is idempotent");
    scope
        .register_operator_group("+".to_string(), second, BindingIndex::value(3))
        .expect("an equal mode + member set is the same record");

    // The first registration's index stands, so the entry stays visible where it was declared.
    assert!(scope
        .bindings()
        .lookup_operator_group("+", Some(2))
        .is_some());
}

/// Upsert: the same probe under a different chaining mode is a conflict — one scope declares one
/// mode per operator. The diagnostic names the probe.
#[test]
fn re_registering_a_conflicting_mode_errors() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);

    let fold = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::FoldLeft));
    let unary = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::Unary));
    scope
        .register_operator_group("+".to_string(), fold, BindingIndex::value(1))
        .unwrap();
    let error = scope
        .register_operator_group("+".to_string(), unary, BindingIndex::value(2))
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
    let region = run_root_storage();
    let root = run_root_bare(&region);

    let module = region
        .brand()
        .alloc_scope(Scope::child_under_module(root, "vec_ops".to_string()));
    let group = region
        .brand()
        .alloc_operator_group(singleton_group("+", ReductionMode::FoldRight));
    module
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .unwrap();

    // Outside the module the probe is undeclared.
    assert!(root.resolve_operator_group_with_chain("+", None).is_none());

    // `USING vec_ops SCOPE (…)`: the window borrows the module's façade over the call site.
    let window = region
        .brand()
        .alloc_scope(Scope::child_transparent(root, module.bindings()));
    let resolved = window
        .resolve_operator_group_with_chain("+", None)
        .expect("the window surfaces the module's registry entry");
    assert_eq!(resolved.mode(), &ReductionMode::FoldRight);
}

/// `register_group_under_all_subsets` installs one entry per nonempty subset, all pointing at the
/// one record, so any probe drawn from the member set resolves the same group.
#[test]
fn subset_registration_covers_every_probe_of_the_member_set() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());
    scope
        .register_group_under_all_subsets(&["+", "-"], group, BindingIndex::value(1))
        .unwrap();

    for probe in ["+", "-", "+ -"] {
        assert!(
            scope
                .resolve_operator_group_with_chain(probe, None)
                .is_some(),
            "the probe `{probe}` must resolve the registered group"
        );
    }
    assert_eq!(probe_key(&["-", "+", "-"]), "+ -");
}

/// The group context an `OP` declaration reads: a `GROUP` body answers with its own record even
/// though it is stamped `Module` (a group is a module), anonymous frames inside it are
/// transparent, and a plain module nested in the group body short-circuits to `None`.
#[test]
fn nearest_group_context_stops_at_a_plain_module() {
    let region = run_root_storage();
    let root = run_root_bare(&region);
    let group = region.brand().alloc_operator_group(arithmetic_group());

    assert!(root.nearest_group_context().is_none());

    let group_scope =
        region
            .brand()
            .alloc_scope(Scope::child_under_group(root, "vec_ops".to_string(), group));
    let in_group = group_scope
        .nearest_group_context()
        .expect("a GROUP body is its own group context");
    assert!(std::ptr::eq(in_group, group));

    // An anonymous frame inside the body (a block, a per-call scope) is transparent.
    let block = region.brand().alloc_scope(group_scope.child_for_call());
    assert!(block
        .nearest_group_context()
        .is_some_and(|g| std::ptr::eq(g, group)));

    // A plain module declared inside the group body is not a group.
    let nested_module = region
        .brand()
        .alloc_scope(Scope::child_under_module(group_scope, "inner".to_string()));
    assert!(nested_module.nearest_group_context().is_none());

    // Nor is a module that never carried a group.
    let plain_module = region
        .brand()
        .alloc_scope(Scope::child_under_module(root, "plain".to_string()));
    assert!(plain_module.nearest_group_context().is_none());
}

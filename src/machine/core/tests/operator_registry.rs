//! Tests for the per-scope operator registry —
//! [`crate::machine::core::Bindings::lookup_operator_group`] and
//! [`crate::machine::core::Scope::resolve_operator_group_with_chain`]. The registry
//! parallels the function/type lookup layers: innermost visible registration wins,
//! a cross-group or undeclared probe misses.

use std::collections::HashMap;

use crate::builtins::test_support::run_root_bare;
use crate::machine::core::{BindingIndex, FrameStorage};
use crate::machine::model::operators::{Associativity, OperatorEntry, OperatorGroup};

/// Arithmetic-shaped group: `+` and `-` both left-associative at one tier.
fn arithmetic_group() -> OperatorGroup {
    let mut members = HashMap::new();
    members.insert(
        "+".to_string(),
        OperatorEntry {
            tier: 10,
            associativity: Associativity::Left,
        },
    );
    members.insert(
        "-".to_string(),
        OperatorEntry {
            tier: 10,
            associativity: Associativity::Left,
        },
    );
    OperatorGroup::new(members)
}

#[test]
fn register_then_resolve_group_by_probe() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let group = region.region().alloc_operator_group(arithmetic_group());
    // A module registers the powerset; "- +" is the sorted-joined probe for a chain
    // mixing both operators.
    scope
        .register_operator_group("- +".to_string(), group, BindingIndex::value(1))
        .unwrap();
    let resolved = scope
        .resolve_operator_group_with_chain("- +", None)
        .expect("registered probe resolves");
    assert!(resolved.entry("+").is_some());
    assert!(resolved.entry("-").is_some());
    assert_eq!(resolved.entry("+").unwrap().tier, 10);
}

#[test]
fn undeclared_probe_misses() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let group = region.region().alloc_operator_group(arithmetic_group());
    scope
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .unwrap();
    // `*` was never registered.
    assert!(scope.resolve_operator_group_with_chain("*", None).is_none());
}

#[test]
fn cross_group_probe_misses() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let group = region.region().alloc_operator_group(arithmetic_group());
    // Only the within-group subsets are registered.
    scope
        .register_operator_group("+".to_string(), group, BindingIndex::value(1))
        .unwrap();
    scope
        .register_operator_group("-".to_string(), group, BindingIndex::value(1))
        .unwrap();
    scope
        .register_operator_group("- +".to_string(), group, BindingIndex::value(1))
        .unwrap();
    // A chain mixing `+` with an operator from a different (unregistered) group
    // produces the probe "+ |", which nothing registered — a clean miss.
    assert!(scope
        .resolve_operator_group_with_chain("+ |", None)
        .is_none());
}

#[test]
fn innermost_scope_shadows_outer() {
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    let inner = region.region().alloc_scope(outer.child_for_call());

    let outer_group = region.region().alloc_operator_group(arithmetic_group());
    let mut inner_members = HashMap::new();
    inner_members.insert(
        "+".to_string(),
        OperatorEntry {
            tier: 99,
            associativity: Associativity::Right,
        },
    );
    let inner_group = region
        .region()
        .alloc_operator_group(OperatorGroup::new(inner_members));

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
    assert_eq!(resolved.entry("+").unwrap().tier, 99);
    assert_eq!(
        resolved.entry("+").unwrap().associativity,
        Associativity::Right
    );

    // From the outer scope, only the outer registration is visible.
    let outer_resolved = outer
        .resolve_operator_group_with_chain("+", None)
        .expect("outer registration resolves");
    assert_eq!(outer_resolved.entry("+").unwrap().tier, 10);
}

#[test]
fn visibility_cutoff_hides_later_sibling_registration() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let group = region.region().alloc_operator_group(arithmetic_group());
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

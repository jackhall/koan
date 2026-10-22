//! The fact vocabulary: a frontier's union and order, and when a binding wakes it.

use crate::memory::{Bump, BumpVec, program_storage};
use crate::type_lattice::KType;

use super::super::{Address, Binding, Bound, Frontier, Segment, Slot, Unknown, merge_into};

fn unknown(segment: u32, slot: u32) -> Unknown {
    Unknown {
        segment: Segment(segment),
        at: Address::Slot(Slot(slot)),
    }
}

#[test]
fn a_frontier_is_sorted_and_deduplicated_and_its_union_merges() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let mut staged = BumpVec::new_in(&scratch);
    staged.extend([unknown(2, 0), unknown(1, 3), unknown(2, 0), unknown(1, 1)]);
    let laid = Frontier::laid(writer, &mut staged);
    assert_eq!(
        laid.unknowns(),
        &[unknown(1, 1), unknown(1, 3), unknown(2, 0)]
    );
    let mut other = BumpVec::new_in(&scratch);
    other.extend([unknown(1, 3), unknown(0, 9)]);
    let other = Frontier::laid(writer, &mut other);
    let union = laid.union(other, writer);
    assert_eq!(
        union.unknowns(),
        &[unknown(0, 9), unknown(1, 1), unknown(1, 3), unknown(2, 0)]
    );
    assert_eq!(laid.union(Frontier::EMPTY, writer), laid);
    assert!(union.contains(unknown(0, 9)) && !union.contains(unknown(0, 8)));
    let mut into = BumpVec::new_in(&scratch);
    merge_into(&mut into, &[unknown(3, 0), unknown(1, 0)]);
    merge_into(&mut into, &[unknown(1, 0), unknown(2, 0)]);
    assert_eq!(&into[..], &[unknown(1, 0), unknown(2, 0), unknown(3, 0)]);
}

#[test]
fn a_binding_wakes_exactly_the_frontiers_naming_what_it_binds() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let mut staged = BumpVec::new_in(&scratch);
    staged.extend([unknown(1, 3), unknown(2, 0)]);
    let frontier = Frontier::laid(writer, &mut staged);
    let laid = |bound: &[(Unknown, KType)]| {
        let mut held = BumpVec::new_in(&scratch);
        held.extend(
            bound
                .iter()
                .map(|(unknown, typed)| (*unknown, Bound::Type(*typed))),
        );
        Binding::laid(writer, &mut held)
    };
    let binding = laid(&[(unknown(2, 0), KType::NUMBER), (unknown(0, 0), KType::STR)]);
    assert!(frontier.meets_binding(binding));
    assert!(!Frontier::EMPTY.meets_binding(binding));
    let apart = laid(&[(unknown(1, 2), KType::NUMBER)]);
    assert!(!frontier.meets_binding(apart));
    assert!(
        matches!(binding.bound(unknown(0, 0)), Some(Bound::Type(typed)) if typed == KType::STR)
    );
    assert!(binding.bound(unknown(1, 3)).is_none());
    assert!(!binding.extends(apart));
    assert!(binding.extends(Binding::EMPTY));
}

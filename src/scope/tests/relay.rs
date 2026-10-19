//! The relay's copy discipline: a relayed tree keeps its identity, a subtree two parts share is
//! copied once, and a fact already relayed is not copied again. Adoption across chains is the
//! dispatch suite's, where a step's scratch chain stands beside its region's.

use crate::memory::{Bump, resident};
use crate::scope::{Binder, Leaf, Relay, Segment, Slot, Tree, Within};
use crate::type_lattice::KType;

use super::with_fixture;

/// An open leaf on slot `slot` of the first body.
fn leaf<'a>(slot: u32) -> Tree<'a> {
    let binder = Binder {
        body: Segment(0),
        slot: Slot(slot),
    };
    Tree::bound(binder, &[], None)
}

#[test]
fn a_relayed_tree_keeps_its_print() {
    with_fixture(|fixture| {
        let staging = Bump::new();
        let source = leaf(0);
        let retyped = Tree::retyped(KType::NUMBER, resident(&staging, source));
        let relay = Relay::new(
            fixture.program.writer(),
            fixture.types,
            fixture.types,
            fixture.scratch(),
        );
        let copy = relay.tree(retyped);
        assert_eq!(copy.print(), retyped.print());
        assert!(copy.same_as(&retyped));
    });
}

#[test]
fn a_subtree_two_captures_share_is_relayed_once() {
    with_fixture(|fixture| {
        let staging = Bump::new();
        let shared = resident(&staging, leaf(0));
        let captures = staging.alloc_slice_copy(&[
            (0, Tree::retyped(KType::NUMBER, shared)),
            (1, Tree::retyped(KType::BOOL, shared)),
        ]);
        let within = resident(&staging, Within::Body(captures));
        let tree = Tree::bound(
            Binder {
                body: Segment(1),
                slot: Slot(0),
            },
            &[],
            Some(within),
        );
        let relay = Relay::new(
            fixture.program.writer(),
            fixture.types,
            fixture.types,
            fixture.scratch(),
        );
        let Tree::Leaf(Leaf::Bound {
            within: Some(Within::Body(copied)),
            ..
        }) = relay.tree(tree)
        else {
            panic!("a leaf within a body's captures");
        };
        let [
            (_, Tree::Retyped { source: a, .. }),
            (_, Tree::Retyped { source: b, .. }),
        ] = copied
        else {
            panic!("two retyped captures");
        };
        assert!(std::ptr::eq(*a, *b), "the shared subtree is copied once");
        assert!(!std::ptr::eq(*a, shared), "the copy is the relay's own");
    });
}

#[test]
fn a_fact_relayed_twice_is_copied_once() {
    with_fixture(|fixture| {
        let staging = Bump::new();
        let source = resident(&staging, leaf(2));
        let relay = Relay::new(
            fixture.program.writer(),
            fixture.types,
            fixture.types,
            fixture.scratch(),
        );
        let copy = relay.tree_ref(source);
        assert!(std::ptr::eq(relay.tree_ref(source), copy));
        assert!(
            std::ptr::eq(relay.tree_ref(copy), copy),
            "a copy relays to itself"
        );
    });
}

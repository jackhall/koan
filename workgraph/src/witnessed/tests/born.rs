//! The born-door slate: what [`RegionHandle::alloc_resident_born`] and
//! [`RegionHandle::alloc_resident_born_with`] store, and that the stored borrows survive the round
//! trip through the typed cell. The family here is the shape koan's `Scope` takes — **invariant**
//! in its region lifetime, naming its own region, and optionally naming a parent that lives in a
//! *different* region — so the erase-store / re-anchor-on-return round trip is exercised under tree
//! borrows on exactly the aliasing pattern production runs.
//!
//! No residence check appears anywhere in this module: the doors discharge residence at the
//! `for<'b>` brand, and the negative case (a value built over an ambient region) is a `compile_fail`
//! doctest on the door itself, not a runtime rejection.

use std::cell::Cell;
use std::ptr;
use std::rc::Rc;

use super::super::*;

/// A stored node in the shape of koan's `Scope`: it names the region it lives in, may name a parent
/// resident in another region, and carries an interior-mutable slot over `'r` — which makes the
/// family **invariant**, the case that actually matters for the re-anchor.
struct Node<'r> {
    home: &'r Region<BornProfile>,
    label: &'r str,
    parent: Option<&'r Node<'r>>,
    mark: Cell<Option<&'r str>>,
}

struct NodeFamily;

reattachable!(NodeFamily => Node<'r>);

/// A bare-reference family, so a parent already stored in another region crosses the brand as a
/// [`SealedExtern`] operand.
struct NodeRefFamily;

reattachable!(NodeRefFamily => &'r Node<'r>);

struct BornProfile;

impl StorageProfile for BornProfile {
    type Families = (NodeFamily, ());
    type FrameOwner = RegionHost<BornProfile>;
}

impl Stored<BornProfile> for NodeFamily {
    fn cell(storage: &StorageOf<BornProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}

type BornFrame = RegionHost<BornProfile>;

fn frame() -> Rc<BornFrame> {
    RegionHost::fresh(None)
}

/// Build a root node in `frame`'s region through the operand-free door — the shape the run-root
/// scope takes.
fn born_root<'a>(frame: &'a Rc<BornFrame>, label: &'static str) -> &'a Node<'a> {
    RegionHandle::from_owner(&**frame).alloc_resident_born::<NodeFamily>(|placement| Node {
        // The only `&Region` reachable at the brand is the placement's own.
        home: placement.handle().region(),
        label: placement.handle().bump_text(label),
        parent: None,
        mark: Cell::new(None),
    })
}

/// The operand-free door stores a value whose region pointer is the destination's, and the stored
/// borrows read back correctly through the re-anchor.
#[test]
fn the_born_door_stores_a_value_naming_its_own_region() {
    let dest = frame();

    let node = born_root(&dest, "root");

    assert!(
        ptr::eq(node.home, dest.region()),
        "the value's region pointer is the destination's by construction"
    );
    assert_eq!(node.label, "root");
    assert!(node.parent.is_none());
    assert_eq!(dest.region().family_len::<NodeFamily>(), 1);
}

/// Reading a stored node back **after further allocations into the same cell** is the arena's
/// append-stable-address guarantee: the first node's pages must not move under the later stores.
#[test]
fn an_earlier_node_reads_back_after_its_siblings_are_stored() {
    let dest = frame();

    let first = born_root(&dest, "first");
    for index in 0..64 {
        let handle = RegionHandle::from_owner(&*dest);
        handle.alloc_resident_born::<NodeFamily>(|placement| Node {
            home: placement.handle().region(),
            label: placement.handle().bump_text(&format!("sibling-{index}")),
            parent: None,
            mark: Cell::new(None),
        });
    }

    assert_eq!(first.label, "first", "the first store survives 64 siblings");
    assert!(ptr::eq(first.home, dest.region()));
    assert_eq!(dest.region().family_len::<NodeFamily>(), 65);
}

/// The interior-mutable slot is written **after** the door returns, at the caller's own `'a` — the
/// shape koan's post-store `RefCell` seeding takes. Invariance makes this the load-bearing case: a
/// covariant family would type-check under a weaker re-anchor.
#[test]
fn the_returned_node_accepts_a_write_at_the_callers_lifetime() {
    let dest = frame();
    let node = born_root(&dest, "root");
    let text: &str = RegionHandle::from_owner(&*dest).bump_text("marked");

    node.mark.set(Some(text));

    assert_eq!(node.mark.get(), Some("marked"));
}

/// The crossing-operand door: a child born in one region embedding a parent resident in **another**,
/// with the parent's frame held as the pin for the destination's whole life. This is the frame-child
/// shape — the one store whose operand is genuinely foreign.
#[test]
fn the_born_with_door_embeds_a_parent_from_another_region() {
    let parent_frame = frame();
    let parent: &Node<'_> = born_root(&parent_frame, "parent");

    let child_frame = frame();
    let child = RegionHandle::from_owner(&*child_frame)
        .alloc_resident_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(parent),
            &parent_frame,
            |placement, parent_at_brand| Node {
                home: placement.handle().region(),
                label: placement.handle().bump_text("child"),
                parent: Some(parent_at_brand),
                mark: Cell::new(None),
            },
        );

    assert!(
        ptr::eq(child.home, child_frame.region()),
        "the child is resident where it was born"
    );
    assert_eq!(child.label, "child");
    let seen = child.parent.expect("the parent crossed the brand");
    assert_eq!(seen.label, "parent");
    assert!(
        ptr::eq(seen.home, parent_frame.region()),
        "and it still names the region it actually lives in"
    );
}

/// The destination region dies **first**, with the pinned source outliving it — the drop order the
/// frame chain guarantees in production. Under Miri's leak check and tree borrows this is the
/// acceptance criterion made observable: the child's cross-region borrow is never read after its own
/// region goes, and the parent's storage is released cleanly afterwards.
#[test]
fn a_child_region_dies_before_the_parent_it_borrows() {
    let parent_frame = frame();
    let parent = born_root(&parent_frame, "parent");

    {
        let child_frame = frame();
        let child = RegionHandle::from_owner(&*child_frame)
            .alloc_resident_born_with::<NodeFamily, NodeRefFamily, _>(
                SealedExtern::<NodeRefFamily>::erase(parent),
                &parent_frame,
                |placement, parent_at_brand| Node {
                    home: placement.handle().region(),
                    label: placement.handle().bump_text("child"),
                    parent: Some(parent_at_brand),
                    mark: Cell::new(None),
                },
            );
        assert_eq!(child.parent.map(|p| p.label), Some("parent"));
        drop(child_frame);
    }

    assert_eq!(
        parent.label, "parent",
        "the parent outlives what borrowed it"
    );
    drop(parent_frame);
}

/// A chain of three regions, each child borrowing the one before it: the transitive shape a nested
/// call stack builds. Every hop reads back through its own re-anchor.
#[test]
fn a_chain_of_regions_reads_back_through_every_hop() {
    let root_frame = frame();
    let root = born_root(&root_frame, "root");

    let middle_frame = frame();
    let middle = RegionHandle::from_owner(&*middle_frame)
        .alloc_resident_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(root),
            &root_frame,
            |placement, parent| Node {
                home: placement.handle().region(),
                label: placement.handle().bump_text("middle"),
                parent: Some(parent),
                mark: Cell::new(None),
            },
        );

    let leaf_frame = frame();
    let leaf = RegionHandle::from_owner(&*leaf_frame)
        .alloc_resident_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(middle),
            &middle_frame,
            |placement, parent| Node {
                home: placement.handle().region(),
                label: placement.handle().bump_text("leaf"),
                parent: Some(parent),
                mark: Cell::new(None),
            },
        );

    let mut walked = Vec::new();
    let mut cursor = Some(leaf);
    while let Some(node) = cursor {
        walked.push(node.label);
        cursor = node.parent;
    }
    assert_eq!(walked, vec!["leaf", "middle", "root"]);
}

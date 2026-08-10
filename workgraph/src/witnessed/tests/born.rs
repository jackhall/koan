//! The bump-residence slate: what a value built at the caller's own `'a` and bumped through
//! [`BumpAllocator::in_place`] stores, what [`RegionHandle::bump_born_with`] stores for a value
//! embedding a *foreign* operand, and that the stored borrows survive both round trips. The family
//! here is the shape koan's `Scope` takes — **invariant** in its region lifetime, naming its own
//! region, and optionally naming a parent that lives in a *different* region — so the re-anchor at
//! the crossing door is exercised under tree borrows on exactly the aliasing pattern production
//! runs.
//!
//! No residence check appears anywhere in this module: the crossing door discharges residence at the
//! `for<'b>` brand, and the negative case (a value built over an ambient region) is a `compile_fail`
//! doctest on the door itself, not a runtime rejection. The same-region cases need no proof at all —
//! their fields are already at `'a`, so the borrow checker is the whole argument.

use std::cell::Cell;
use std::ptr;
use std::rc::Rc;

use super::super::*;

/// A stored node in the shape of koan's `Scope`: it names the region it lives in, may name a parent
/// resident in another region, and carries an interior-mutable slot over `'r` — which makes the
/// family **invariant**, the case that actually matters for the re-anchor, and rules `Copy` out,
/// which is why it is bumped through `in_place` rather than `value`.
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

/// The region stores nothing typed: every node here lives in the bump, which is the whole point.
struct BornProfile;

impl StorageProfile for BornProfile {
    type Families = ();
    type FrameOwner = RegionHost<BornProfile>;
}

type BornFrame = RegionHost<BornProfile>;

fn frame() -> Rc<BornFrame> {
    RegionHost::fresh(None)
}

/// Build a root node in `frame`'s region — the same-region shape, and the reason the operand-free
/// born door has no bump twin: every field is already at `'a`, so the value is built there and
/// bumped with no brand at all.
fn bumped_root<'a>(frame: &'a Rc<BornFrame>, label: &'static str) -> &'a Node<'a> {
    let handle = RegionHandle::from_owner(&**frame);
    handle.allocator().in_place(Node {
        home: handle.region(),
        label: handle.allocator().text(label),
        parent: None,
        mark: Cell::new(None),
    })
}

/// A same-region value is born at `'a` naming its own region, and its bumped borrows read back.
#[test]
fn a_same_region_value_is_bumped_naming_its_own_region() {
    let dest = frame();

    let node = bumped_root(&dest, "root");

    assert!(
        ptr::eq(node.home, dest.region()),
        "the value's region pointer is the one it was built over"
    );
    assert_eq!(node.label, "root");
    assert!(node.parent.is_none());
}

/// Reading a stored node back **after further allocations into the same bump** is bumpalo's
/// chunk-stability guarantee: the first node must not move under the later stores, even across the
/// chunk growth 64 siblings force.
#[test]
fn an_earlier_node_reads_back_after_its_siblings_are_stored() {
    let dest = frame();

    let first = bumped_root(&dest, "first");
    let before = dest.region().bump_capacity();
    for index in 0..64 {
        let handle = RegionHandle::from_owner(&*dest);
        handle.allocator().in_place(Node {
            home: handle.region(),
            label: handle.allocator().text(&format!("sibling-{index}")),
            parent: None,
            mark: Cell::new(None),
        });
    }

    assert_eq!(first.label, "first", "the first store survives 64 siblings");
    assert!(ptr::eq(first.home, dest.region()));
    assert!(
        dest.region().bump_capacity() > before,
        "the siblings really landed in the bump"
    );
}

/// The interior-mutable slot is written **after** the value is bumped, at the caller's own `'a`,
/// through the shared `&` the bump hands back — the shape `in_place` exists for and the one `Copy`
/// cannot express. Invariance makes this the load-bearing case: a covariant family would type-check
/// under a weaker anchor. The region then dies with the mutation still resident, which is the leak
/// claim the `!needs_drop` assert stands for.
#[test]
fn a_bumped_value_accepts_writes_through_the_shared_reference() {
    let dest = frame();
    let node = bumped_root(&dest, "root");
    let handle = RegionHandle::from_owner(&*dest);
    let text: &str = handle.allocator().text("marked");

    node.mark.set(Some(text));
    assert_eq!(node.mark.get(), Some("marked"));

    // Overwrite with a second bumped string: the slot keeps mutating in place for the region's life.
    node.mark.set(Some(handle.allocator().text("remarked")));
    assert_eq!(node.mark.get(), Some("remarked"));

    drop(dest);
}

/// The crossing-operand door: a child born in one region embedding a parent resident in **another**,
/// with the parent's frame held as the pin for the destination's whole life. This is the frame-child
/// shape — the one store whose operand is genuinely foreign.
#[test]
fn the_crossing_door_embeds_a_parent_from_another_region() {
    let parent_frame = frame();
    let parent: &Node<'_> = bumped_root(&parent_frame, "parent");

    let child_frame = frame();
    let child = RegionHandle::from_owner(&*child_frame)
        .bump_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(parent),
            &parent_frame,
            |placement, parent_at_brand| Node {
                home: placement.handle().region(),
                label: placement.handle().allocator().text("child"),
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

/// The crossing door pinned by the **destination's own host** rather than the source's: the child
/// host names the parent as its `outer`, so holding the child holds the parent's region too, and the
/// caller's own handle on the parent can go the moment the store returns. This is the frame chain as
/// a pin — the shape a per-call frame takes, where the only `Rc` an embedder keeps is the innermost
/// one and every ancestor region rides its `outer` links. A regression that dropped the chain leaves
/// the embedded parent dangling the instant the local handle goes, which is the read below.
#[test]
fn the_crossing_door_accepts_the_childs_own_host_as_the_pin() {
    let parent_frame = frame();
    let parent_alive = Rc::downgrade(&parent_frame);
    let child_frame: Rc<BornFrame> = RegionHost::fresh(Some(Rc::clone(&parent_frame)));

    let child = {
        let parent: &Node<'_> = bumped_root(&parent_frame, "parent");
        RegionHandle::from_owner(&*child_frame).bump_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(parent),
            // The pin is the destination's own host — it covers the operand only through the
            // `outer` chain, which is exactly the claim under test.
            &child_frame,
            |placement, parent_at_brand| Node {
                home: placement.handle().region(),
                label: placement.handle().allocator().text("child"),
                parent: Some(parent_at_brand),
                mark: Cell::new(None),
            },
        )
    };

    // Every direct handle on the parent goes; the child's `outer` link is the sole pin left.
    drop(parent_frame);
    assert!(
        parent_alive.upgrade().is_some(),
        "the child host's `outer` link keeps the parent region alive on its own"
    );

    let seen = child.parent.expect("the parent crossed the brand");
    assert_eq!(seen.label, "parent");
    assert!(
        !ptr::eq(seen.home, child_frame.region()),
        "the embedded parent still names the region it actually lives in"
    );

    drop(child_frame);
    assert!(
        parent_alive.upgrade().is_none(),
        "and the chain releases it when the child goes — no cycle either way"
    );
}

/// A resident node erased to the witness-less [`SealedExtern`] carrier — the lifetime-free slot
/// shape an embedder's scheduler stores — opens at a `for<'b>` brand under the frame's own pin,
/// and the region **grows through the bump while the opened reference is live**: one region under
/// the re-anchored view and a sibling store at once, re-read on both sides of the append.
#[test]
fn an_erased_node_opens_and_survives_a_sibling_store_inside_the_open() {
    let dest = frame();
    let node = bumped_root(&dest, "kept");

    let sealed = SealedExtern::<NodeRefFamily>::erase(node);
    sealed.open(&dest, |reattached: &Node<'_>| {
        assert_eq!(reattached.label, "kept");
        bumped_root(&dest, "sibling");
        assert_eq!(
            reattached.label, "kept",
            "the opened view re-reads across the sibling store"
        );
    });
}

/// The destination region dies **first**, with the pinned source outliving it — the drop order the
/// frame chain guarantees in production. Under Miri's leak check and tree borrows this is the
/// acceptance criterion made observable: the child's cross-region borrow is never read after its own
/// region goes, and the parent's storage is released cleanly afterwards.
#[test]
fn a_child_region_dies_before_the_parent_it_borrows() {
    let parent_frame = frame();
    let parent = bumped_root(&parent_frame, "parent");

    {
        let child_frame = frame();
        let child = RegionHandle::from_owner(&*child_frame)
            .bump_born_with::<NodeFamily, NodeRefFamily, _>(
                SealedExtern::<NodeRefFamily>::erase(parent),
                &parent_frame,
                |placement, parent_at_brand| Node {
                    home: placement.handle().region(),
                    label: placement.handle().allocator().text("child"),
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
    let root = bumped_root(&root_frame, "root");

    let middle_frame = frame();
    let middle = RegionHandle::from_owner(&*middle_frame)
        .bump_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(root),
            &root_frame,
            |placement, parent| Node {
                home: placement.handle().region(),
                label: placement.handle().allocator().text("middle"),
                parent: Some(parent),
                mark: Cell::new(None),
            },
        );

    let leaf_frame = frame();
    let leaf = RegionHandle::from_owner(&*leaf_frame)
        .bump_born_with::<NodeFamily, NodeRefFamily, _>(
            SealedExtern::<NodeRefFamily>::erase(middle),
            &middle_frame,
            |placement, parent| Node {
                home: placement.handle().region(),
                label: placement.handle().allocator().text("leaf"),
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

/// The region's owner read off the region itself — the back-link a resident value derives its frame
/// from instead of carrying a `Weak` copy of its own. Upgrading it recovers the very host that
/// minted the handle, and the link releases with the host.
#[test]
fn the_handle_reads_the_regions_own_host_back_link() {
    let host = frame();
    let handle = RegionHandle::from_owner(&*host);

    let derived = handle.host();
    let upgraded = derived.upgrade().expect("a live region has a live host");
    assert!(
        Rc::ptr_eq(&upgraded, &host),
        "the back-link names the owner that holds the region"
    );

    drop(upgraded);
    drop(host);
    assert!(
        derived.upgrade().is_none(),
        "and it is weak — the back-link pins nothing"
    );
}

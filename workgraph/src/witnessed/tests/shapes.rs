//! Miri slate (tree borrows) for the abstract carrier shapes the witnessed substrate admits —
//! the reference-only [`Carrier`]'s two liveness channels (residence and reach), the
//! `Residence` × `borrows_host` materialization matrix, envelope duplication, the
//! [`RegionSet::mint`] home-omission rule, and the [`StepContext::alloc_with`] finish-surface
//! fold. Everything routes production verbs over a library-only profile ([`ShapeProfile`] /
//! `RegionHost` frames, `u32` content) — no embedder type. Each test frees every frame handle a
//! regression would leave the value dangling into, then reads the value back: a use-after-free
//! under tree borrows the instant a mint under-counts, and a leak the instant a release
//! over-counts. Fails on UB / leaks, not values.

use std::rc::Rc;

use super::super::*;

/// The library-only storage profile the shape slate runs over: owned `u32` content plus the
/// minted reach sets. `RefValFamily` and the fold families are carrier-only (never stored), so
/// they need no cell.
struct ShapeProfile;

impl StorageProfile for ShapeProfile {
    type Families = (ValFamily, (RegionSet<ShapeFrame>, ()));
}

/// The frame type: the library's own region owner with lazy mint and `outer`-chain pins — the
/// same shape every embedder's frame storage wraps or aliases.
type ShapeFrame = RegionHost<ShapeProfile>;

/// Owned scalar content — what a region stores.
struct ValFamily;
/// A borrow into some region's stored content — the value shape whose liveness the carriers
/// under test must account for.
struct RefValFamily;
/// Aggregate-fold accumulator: the dest handle plus the element borrows folded so far.
struct PairAcc;
/// The finished two-element read carrier (`Copy`, so it reads back through `with_pinned`).
struct PairVals;

reattachable! {
    ValFamily => u32,
    RefValFamily => &'r u32,
    PairAcc => (RegionHandle<'r, ShapeProfile>, Vec<&'r u32>),
    PairVals => (&'r u32, &'r u32),
}

impl Stored<ShapeProfile> for ValFamily {
    fn cell(storage: &StorageOf<ShapeProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}

impl Stored<ShapeProfile> for RegionSet<ShapeFrame> {
    fn cell(storage: &StorageOf<ShapeProfile>) -> &FamilyArena<Self> {
        &storage.1 .0
    }
}

fn frame() -> Rc<ShapeFrame> {
    RegionHost::fresh(None)
}

/// Store `v` into `frame`'s region and hand back the co-located borrow.
fn store_val(frame: &Rc<ShapeFrame>, v: u32) -> &u32 {
    RegionHandle::from_owner(&**frame).alloc_resident::<ValFamily>(v)
}

/// A destination accumulator born through the step context: the dest frame's own handle under the
/// empty reference-only carrier — the `HasRegionHandle` operand every `transfer_into` composes
/// against.
fn dest_handle_acc(
    dest: &Rc<ShapeFrame>,
) -> Witnessed<RegionHandleFamily<ShapeProfile>, Carrier<ShapeFrame>> {
    StepContext::new(Rc::clone(dest))
        .alloc_handle::<ShapeProfile, RegionHandleFamily<ShapeProfile>>(|handle| handle)
}

/// An element whose liveness rides its **reach**, not its residence: the value lives in
/// `content`'s region, the carrier references a reach set naming `content` minted into `host`'s
/// arena ([`Carrier::new`], the entry-re-read constructor), and the envelope host is `host`. When
/// `host` is the consuming destination itself (the defined-in-current-scope shape), home-omission
/// drops the host member at the fold and the reach union alone pins `content`.
fn reach_element(
    host: &Rc<ShapeFrame>,
    content: &Rc<ShapeFrame>,
    v: u32,
) -> Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> {
    let value: &u32 = store_val(content, v);
    let reach = RegionSet::mint(
        RegionHandle::from_owner(&**host),
        &[&RegionSet::singleton(Rc::clone(content))],
        &[],
        |_| false,
    );
    Delivered::seal(
        Witnessed::from_erased(Erased::erase(value), Carrier::new(false, reach)),
        Rc::clone(host),
    )
}

/// **Residence channel, `Kept`** — a value that keeps living in its producer's region rides the
/// envelope host, which a `Kept` fold materializes unconditionally into the destination's minted
/// set. The producer handle drops; the minted member is the sole pin on the region the read
/// dereferences into.
#[test]
fn kept_transfer_materializes_residence_host() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 7)),
        Rc::clone(&producer),
    );
    let dest = frame();
    let merged: Witnessed<RefValFamily, Carrier<ShapeFrame>> = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            Residence::Kept,
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.with_pinned(&dest, |r| **r), 7);
}

/// **Reach channel across chained folds** — two elements hosted by the destination itself (the
/// defined-in-current-scope shape: home-omission drops the host member, so residence
/// materialization contributes nothing) whose carriers reach two independently-dying content
/// regions. Each fold must union the element's reach onto the accumulator's minted set — and the
/// second fold must re-mint the first's members (`compose_into` composes both operands, never the
/// newcomer alone). Every content handle drops; the destination's minted set is the sole pin on
/// both regions when the pair reads back.
#[test]
fn kept_transfer_unions_element_reach_across_folds() {
    let dest = frame();
    let content_a = frame();
    let content_b = frame();

    let acc0: Witnessed<PairAcc, Carrier<ShapeFrame>> =
        StepContext::new(Rc::clone(&dest))
            .alloc_handle::<ShapeProfile, PairAcc>(|handle| (handle, Vec::new()));
    let acc1 = reach_element(&dest, &content_a, 1).transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc0,
        Residence::Kept,
        |value, (handle, mut values), _brand| {
            values.push(value);
            (handle, values)
        },
    );
    let acc2 = reach_element(&dest, &content_b, 2).transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc1,
        Residence::Kept,
        |value, (handle, mut values), _brand| {
            values.push(value);
            (handle, values)
        },
    );
    let pair: Witnessed<PairVals, Carrier<ShapeFrame>> =
        acc2.map_pinned(&dest, |(_handle, values), _brand| (values[0], values[1]));

    drop(content_a);
    drop(content_b);

    assert_eq!(pair.with_pinned(&dest, |(a, b)| (**a, **b)), (1, 2));
}

/// **Residence channel, `Copied` × `borrows_host` set** — the relocated product still borrows
/// into the producer's region (the closure-like value), so the `Copied` fold must materialize the
/// host off the `borrows_host` bit. The producer handle drops; the bit-driven member is the sole
/// pin under the read.
#[test]
fn copied_transfer_materializes_borrowing_host() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::from_erased(
            Erased::erase(store_val(&producer, 5)),
            Carrier::new(true, None),
        ),
        Rc::clone(&producer),
    );
    let dest = frame();
    let merged: Witnessed<RefValFamily, Carrier<ShapeFrame>> = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            Residence::Copied,
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.with_pinned(&dest, |r| **r), 5);
}

/// **Residence channel, `Copied` × `borrows_host` unset — the release half.** A true deep copy
/// leaves no borrow into the producer, so the fold must NOT materialize the residence-only host:
/// once the envelope and the producer handle drop, the producer's region genuinely frees (the
/// tail-loop turnover rule) while the copy stays readable in the destination. A phantom member
/// here is the leak this test gates.
#[test]
fn copied_transfer_releases_residence_only_host() {
    let producer = frame();
    let weak = Rc::downgrade(&producer);
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 9)),
        Rc::clone(&producer),
    );
    let dest = frame();
    let copied: Witnessed<ValFamily, Carrier<ShapeFrame>> = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, ValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            Residence::Copied,
            |value, _handle, _brand| *value,
        );
    drop(element);
    drop(producer);
    assert!(
        weak.upgrade().is_none(),
        "a residence-only host is released with its envelope, never minted"
    );
    assert_eq!(copied.with_pinned(&dest, |v| *v), 9);
}

/// **Envelope duplication mints nothing** — duplicating for another consumer bit-copies the
/// reference-only carrier (the reach set rides by reference, never re-minted) and clones exactly
/// one `Rc`, the retained host. Per-member refcount traffic or a re-mint here is the regression
/// this gates; the leak detector is the backstop.
#[test]
fn duplicate_shares_reach_and_clones_one_host() {
    let home = frame();
    let content = frame();
    let element = reach_element(&home, &content, 4);
    let reach_ptr: *const RegionSet<ShapeFrame> =
        element.witness().with_reach(Some(element.host()), |reach| {
            reach.expect("the element carries a minted reach") as *const _
        });
    let home_count = Rc::strong_count(&home);
    let content_count = Rc::strong_count(&content);

    let first = element.duplicate();
    let second = element.duplicate();
    for duplicate in [&first, &second] {
        let ptr = duplicate
            .witness()
            .with_reach(Some(duplicate.host()), |reach| {
                reach.expect("duplicates carry the reach") as *const _
            });
        assert_eq!(ptr, reach_ptr, "the reach set rides by reference");
    }
    assert_eq!(
        Rc::strong_count(&home),
        home_count + 2,
        "one retained-host clone per duplicate"
    );
    assert_eq!(
        Rc::strong_count(&content),
        content_count,
        "no per-member refcount traffic"
    );
}

/// **`RegionSet::mint` home-omission** — a set hosted in region A never holds `Rc<A>` (the
/// self-cycle rule): minting sources that include the destination's own frame materializes only
/// the foreign member, the destination's arena then being that member's sole owner. Dropping the
/// destination releases everything — the Miri leak audit over this test is what signs off the
/// no-self-cycle shape.
#[test]
fn mint_home_omission_prevents_self_cycle() {
    let a = frame();
    let b = frame();
    let weak_a = Rc::downgrade(&a);
    let weak_b = Rc::downgrade(&b);

    let minted = RegionSet::mint(
        RegionHandle::from_owner(&*a),
        &[
            &RegionSet::singleton(Rc::clone(&a)),
            &RegionSet::singleton(Rc::clone(&b)),
        ],
        &[],
        |_| false,
    )
    .expect("the foreign member materializes");
    assert!(
        matches!(minted.members(), [only] if Rc::ptr_eq(only, &b)),
        "home is omitted; the foreign member is kept"
    );

    drop(b);
    assert!(
        weak_b.upgrade().is_some(),
        "a's arena holds the sole surviving member"
    );
    drop(a);
    assert!(weak_a.upgrade().is_none(), "no self-cycle: a freed on drop");
    assert!(weak_b.upgrade().is_none(), "teardown released the member");
}

/// **Finish-surface fold** — `alloc_with` folds every listed dep's envelope into the result's
/// carrier *by construction*, before the build closure can embed a dep view. The built value here
/// IS a dep view (a borrow into the producer's region, riding the result un-copied); the producer
/// handle drops, and the by-construction fold is the sole pin under the read — the mirror of the
/// behavioral membership test above it in `tests.rs`, UAF-shaped.
#[test]
fn alloc_with_folds_dep_reach_before_result_read() {
    let dep_frame = frame();
    let dep: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&dep_frame, 3)),
        Rc::clone(&dep_frame),
    );
    let own = frame();
    let ctx: StepContext<ShapeFrame> = StepContext::new(Rc::clone(&own));
    let built: Witnessed<RefValFamily, Carrier<ShapeFrame>> = ctx
        .alloc_with::<RefValFamily, RefValFamily, ShapeProfile>(&[&dep], |_region, views| views[0]);
    drop(dep);
    drop(dep_frame);
    assert_eq!(built.with_pinned(&own, |r| **r), 3);
}

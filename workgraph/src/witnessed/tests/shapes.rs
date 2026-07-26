//! Miri slate (tree borrows) for the abstract carrier shapes the witnessed substrate admits —
//! home riding a delivery envelope's pins as an ordinary member, the copy-versus-pin choice a
//! relocation site makes through its source-pins claim, envelope duplication, the
//! [`ReachDescription::mint`] self rule, the three carrier states and the transform verbs between
//! them, and the [`StepContext::alloc_with`] finish-surface fold. Everything routes production
//! verbs over a library-only profile ([`ShapeProfile`] /
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
    type Families = (ValFamily, ());
    type FrameOwner = ShapeFrame;
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

/// An element whose value lives in a region other than its home: the value lives in `content`'s
/// region, the carrier references a reach set naming `content` minted into `home`'s arena
/// ([`Carrier::new`], the entry-re-read constructor), and the envelope's pins are `home` ∪ that
/// set. When `home` is the consuming destination itself (the defined-in-current-scope shape), the
/// self rule drops the home member from the fold's bundle and the reach union alone pins `content`.
fn reach_element(
    home: &Rc<ShapeFrame>,
    content: &Rc<ShapeFrame>,
    v: u32,
) -> Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> {
    let value: &u32 = store_val(content, v);
    // `content` is foreign to `home`, so it survives the mint into `home`'s arena; the returned
    // bundle is the value's owned reach, threaded into the envelope at `seal` (which unions `home`
    // in as an ordinary member) so each holder owns its pins.
    let (reach, bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&**home),
        &[&PinBundle::singleton(Rc::clone(content))],
        |_| false,
    );
    Delivered::seal(
        Witnessed::from_erased(Erased::erase(value), Carrier::new(false, reach)),
        Rc::clone(home),
        bundle,
    )
}

/// **Home as an ordinary member** — a value that keeps living in its producer's region rides that
/// region in the envelope's own pins, so a transfer claiming those pins composes the producer into
/// the destination's minted set with no residence mode to name. The producer handle drops; the
/// minted member is the sole pin on the region the read dereferences into.
#[test]
fn transfer_composes_the_source_home_from_its_pins() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 7)),
        Rc::clone(&producer),
        PinBundle::empty(),
    );
    let dest = frame();
    let (merged, _bundle): (Witnessed<RefValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
            element.pins(),
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.with_pinned(&dest, |r| **r), 7);
}

/// **Reach across chained folds** — two elements homed in the destination itself (the
/// defined-in-current-scope shape: the self rule drops the home member, so home contributes
/// nothing) whose carriers reach two independently-dying content regions. Each fold must union the
/// element's reach onto the accumulator's minted set — and the second fold must re-mint the first's
/// members (`compose_into` composes both operands, never the newcomer alone). Every content handle
/// drops; the destination's minted set is the sole pin on both regions when the pair reads back.
#[test]
fn transfer_unions_element_reach_across_folds() {
    let dest = frame();
    let content_a = frame();
    let content_b = frame();

    let acc0: Witnessed<PairAcc, Carrier<ShapeFrame>> =
        StepContext::new(Rc::clone(&dest))
            .alloc_handle::<ShapeProfile, PairAcc>(|handle| (handle, Vec::new()));
    let element_a = reach_element(&dest, &content_a, 1);
    let (acc1, acc1_bundle) = element_a.transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc0,
        &PinBundle::empty(),
        element_a.pins(),
        |value, (handle, mut values), _brand| {
            values.push(value);
            (handle, values)
        },
    );
    let element_b = reach_element(&dest, &content_b, 2);
    let (acc2, _acc2_bundle) = element_b.transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc1,
        &acc1_bundle,
        element_b.pins(),
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

/// **The copy that still borrows** — the relocated product is a copy whose leaves still point into
/// the producer's region (the closure-like value), so the site claims the envelope's own pins and
/// the fold composes the producer in. The producer handle drops; the composed member is the sole
/// pin under the read.
#[test]
fn copied_transfer_pins_the_producer_when_the_product_still_borrows() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::from_erased(
            Erased::erase(store_val(&producer, 5)),
            Carrier::new(true, None),
        ),
        Rc::clone(&producer),
        PinBundle::empty(),
    );
    let dest = frame();
    let (merged, _bundle): (Witnessed<RefValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
            element.pins(),
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.with_pinned(&dest, |r| **r), 5);
}

/// **The release half — a true deep copy.** The copy leaves no borrow into the producer, so the
/// site claims the **empty** bundle and the fold pins nothing on the source side: once the envelope
/// and the producer handle drop, the producer's region genuinely frees (the tail-loop turnover
/// rule) while the copy stays readable in the destination. A phantom member here is the leak this
/// test gates, and it is what an unconditional "home is always a member" fold would produce.
#[test]
fn copied_transfer_releases_the_producer_when_nothing_borrows_it() {
    let producer = frame();
    let weak = Rc::downgrade(&producer);
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 9)),
        Rc::clone(&producer),
        PinBundle::empty(),
    );
    let dest = frame();
    let (copied, _bundle): (Witnessed<ValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, ValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
            &PinBundle::empty(),
            |value, _handle, _brand| *value,
        );
    drop(element);
    drop(producer);
    assert!(
        weak.upgrade().is_none(),
        "an unclaimed producer is released with its envelope, never minted"
    );
    assert_eq!(copied.with_pinned(&dest, |v| *v), 9);
}

/// **Envelope duplication shares the description, clones the owned pins** — duplicating for another
/// consumer bit-copies the reference-only carrier, so the reach **description** rides by reference
/// (never re-minted); but the envelope owns its liveness now, so each duplicate clones the whole
/// [`PinBundle`] — one `Rc` per member, home among them — giving every fan-out consumer its own
/// pins for the parked period. A re-mint of the description here is the regression this gates; the
/// leak detector is the backstop.
#[test]
fn duplicate_shares_reach_and_clones_owned_pins() {
    let home = frame();
    let content = frame();
    let element = reach_element(&home, &content, 4);
    let reach_ptr: *const ReachDescription<ShapeFrame> = element
        .open_at()
        .with_reach(|reach| reach.expect("the element carries a minted reach") as *const _);
    let home_count = Rc::strong_count(&home);
    let content_count = Rc::strong_count(&content);

    let first = element.duplicate();
    let second = element.duplicate();
    for duplicate in [&first, &second] {
        let ptr = duplicate
            .open_at()
            .with_reach(|reach| reach.expect("duplicates carry the reach") as *const _);
        assert_eq!(ptr, reach_ptr, "the reach description rides by reference");
    }
    assert_eq!(
        Rc::strong_count(&home),
        home_count + 2,
        "home is an ordinary bundle member — one clone per duplicate"
    );
    assert_eq!(
        Rc::strong_count(&content),
        content_count + 2,
        "one owned pin clone per duplicate — each consumer owns its pins"
    );
}

/// **`ReachDescription::mint`'s self rule** — a description hosted in region A *does* name `A`
/// (membership is exact, so a later lift re-owns it), but the **owned bundle** minted alongside it
/// does not: a region holding an `Rc` on its own owner is a cycle that frees neither. Dropping the
/// bundle releases the foreign member; dropping A frees A, proving no self-cycle — the Miri leak
/// audit over this test signs off the split-membership shape.
#[test]
fn mint_keeps_home_in_the_description_but_not_the_bundle() {
    let a = frame();
    let b = frame();
    let weak_a = Rc::downgrade(&a);
    let weak_b = Rc::downgrade(&b);

    let source = PinBundle::union(
        &PinBundle::singleton(Rc::clone(&a)),
        &PinBundle::singleton(Rc::clone(&b)),
    );
    let (minted, bundle) =
        ReachDescription::mint(RegionHandle::from_owner(&*a), &[&source], |_| false);
    let minted = minted.expect("both members compose");
    assert!(
        minted.pins_region(a.region()) && minted.pins_region(b.region()),
        "membership is exact: the description names the destination's own region too"
    );
    assert!(
        matches!(bundle.members(), [only] if Rc::ptr_eq(only, &b)),
        "the self rule strips A from the owned bundle; the foreign member stays"
    );
    drop(source);

    drop(b);
    assert!(
        weak_b.upgrade().is_some(),
        "the minted bundle holds the sole surviving member (A's side table only names it weakly)"
    );
    drop(a);
    assert!(weak_a.upgrade().is_none(), "no self-cycle: a freed on drop");
    // The member is owned by the bundle now, not A's arena, so freeing A does not release it.
    drop(bundle);
    assert!(
        weak_b.upgrade().is_none(),
        "dropping the pin bundle released the member"
    );
}

/// **`Delivered::lift`** — the `Sealed → Delivered` transform re-owns the sealed carrier's reach
/// description (`Weak → Rc`) into an owned inline bundle under the host pin, so the value survives
/// its description's hosting arena dying in transit. A seal whose carrier references a set naming
/// `content` (hosted in `host`) is lifted; the lifted bundle owns `content`, so dropping the `host`
/// handle (the description's arena) leaves the value readable. A missed upgrade is a dangling `Weak`
/// read — a UAF under tree borrows.
#[test]
fn lift_reowns_description_into_transit_bundle() {
    let host = frame();
    let content = frame();
    let value: &u32 = store_val(&content, 5);
    let (reach, _bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&*host),
        &[&PinBundle::singleton(Rc::clone(&content))],
        |_| false,
    );
    let sealed: Sealed<RefValFamily, Carrier<ShapeFrame>> = Sealed::seal(Witnessed::from_erased(
        Erased::erase(value),
        Carrier::new(false, reach),
    ));

    let delivered = Delivered::lift(sealed, Rc::clone(&host));
    assert!(
        delivered.pins().pins_region(content.region()),
        "the lift re-owns the description's member into the transit bundle"
    );
    assert!(
        delivered.pins().pins_region(host.region()),
        "and unions home in as an ordinary member of the same bundle"
    );
    // Drop the description's hosting arena; the lifted owned bundle keeps `content` (the value's
    // backing) alive on its own.
    drop(host);
    assert_eq!(delivered.open(|r| *r), 5);
    drop(content);
}

/// **`Delivered::adopt`** — the `Delivered → Sealed` transform mints the value's reach into `dest`
/// and hands the owned bundle back to the holder that adopted it, re-sealing the value resident. A
/// value living in its producer is adopted into `dest`; after the producer handle drops, the
/// returned bundle (standing in for the scope's union bundle) is the sole pin under which the
/// resident seal reads back. A bundle that failed to name the producer is a UAF here.
#[test]
fn adopt_settles_resident_value_into_dest() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 7)),
        Rc::clone(&producer),
        PinBundle::empty(),
    );
    let dest = frame();
    let (sealed, pins): (
        Sealed<RefValFamily, Carrier<ShapeFrame>>,
        PinBundle<ShapeFrame>,
    ) = element.adopt(RegionHandle::from_owner(&*dest), |_| false);
    drop(element);
    drop(producer);
    assert_eq!(sealed.open_with(&pins, |r| *r), 7);
}

/// **The three states and the four transform verbs, end to end** — `Delivered → adopt → Sealed →
/// open_at → Opened → reseal → Sealed → lift → Delivered`, with every intermediate handle dropped
/// before the final read. The value lives in `producer`'s region throughout; the only thing keeping
/// that region alive after the drops is the chain of pins each verb hands to the next, so a verb
/// that loses a member is a use-after-free here and one that gains a phantom member is a leak.
#[test]
fn transform_verb_round_trip_preserves_liveness() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident(store_val(&producer, 11)),
        Rc::clone(&producer),
        PinBundle::empty(),
    );
    let dest = frame();

    // Delivered → Sealed: at rest in `dest`'s table, its pins held by the adopting holder.
    let (sealed, pins) = element.adopt(RegionHandle::from_owner(&*dest), |_| false);
    assert!(
        sealed.open_at(&pins).reach_covers(producer.region()),
        "adopt composed the value's home into dest's description as an ordinary member"
    );
    drop(element);
    drop(producer);

    // Sealed → Opened → Sealed: the in-use state answers membership, then returns to rest.
    let opened = sealed.open_at(&pins);
    assert_eq!(*opened.value(), 11);
    let resealed = opened.reseal();

    // Sealed → Delivered: the lift re-owns the description into a transit bundle of its own, so the
    // holder's pins can go.
    let delivered = Delivered::lift(resealed, Rc::clone(&dest));
    drop(pins);
    drop(dest);
    assert_eq!(delivered.open(|r| *r), 11);
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
        PinBundle::empty(),
    );
    let own = frame();
    let ctx: StepContext<ShapeFrame> = StepContext::new(Rc::clone(&own));
    let (built, _bundle): (Witnessed<RefValFamily, Carrier<ShapeFrame>>, _) = ctx
        .alloc_with::<RefValFamily, RefValFamily, ShapeProfile>(
            &[&dep],
            |_region, views, _token| views[0],
        );
    drop(dep);
    drop(dep_frame);
    assert_eq!(built.with_pinned(&own, |r| **r), 3);
}

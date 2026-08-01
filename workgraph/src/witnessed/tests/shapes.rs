//! Miri slate (tree borrows) for the abstract carrier shapes the witnessed substrate admits —
//! home riding a delivery envelope's pins as an ordinary member, the copy-versus-pin choice a
//! relocation site makes through its source-pins claim, envelope duplication, the
//! [`ReachDescription::mint`] self rule, the three carrier states and the transform verbs between
//! them, the drop to rest that lodges a value's coverage in a region so its cell rides as plain
//! data, and the [`StepContext::alloc_with`] finish-surface fold. Everything routes production
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
) -> Delivered<RegionHandleFamily<ShapeProfile>, Carrier<ShapeFrame>, ShapeFrame> {
    Delivered::seal(
        StepContext::new(Rc::clone(dest))
            .alloc_handle::<ShapeProfile, RegionHandleFamily<ShapeProfile>>(|handle| handle),
        Rc::clone(dest),
        StepCoverage::empty(),
    )
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
    );
    Delivered::seal(
        Witnessed::from_erased(Erased::erase(value), Carrier::new(reach)),
        Rc::clone(home),
        StepCoverage(bundle),
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
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 7), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();
    let merged = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            // The product IS the source borrow, so it still reaches every region the envelope pins.
            |_product, _region| true,
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.open(|r| *r), 7);
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

    let acc0 = Delivered::seal(
        StepContext::new(Rc::clone(&dest))
            .alloc_handle::<ShapeProfile, PairAcc>(|handle| (handle, Vec::new())),
        Rc::clone(&dest),
        StepCoverage::empty(),
    );
    let element_a = reach_element(&dest, &content_a, 1);
    let acc1 = element_a.transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc0,
        |_product, _region| true,
        |value, (handle, mut values), _brand| {
            values.push(value);
            (handle, values)
        },
    );
    let element_b = reach_element(&dest, &content_b, 2);
    let acc2 = element_b.transfer_into::<PairAcc, PairAcc, ShapeProfile>(
        acc1,
        |_product, _region| true,
        |value, (handle, mut values), _brand| {
            values.push(value);
            (handle, values)
        },
    );
    let pair: Witnessed<PairVals, Carrier<ShapeFrame>> = acc2
        .into_cell()
        .unseal()
        .map_pinned(&dest, |(_handle, values), _brand| (values[0], values[1]));

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
    // The value borrows into its own birth region, so the mint composes `producer` in as an
    // ordinary member — the shape a substrate value born at a fold door carries.
    let value = store_val(&producer, 5);
    let (reach, bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&*producer),
        &[&PinBundle::singleton(Rc::clone(&producer))],
    );
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::from_erased(Erased::erase(value), Carrier::new(reach)),
        Rc::clone(&producer),
        StepCoverage(bundle),
    );
    let dest = frame();
    let merged = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            // The product's leaves still point into the producer's region, so the predicate keeps
            // every member and the fold composes the producer in.
            |_product, _region| true,
            |value, _handle, _brand| value,
        );
    drop(element);
    drop(producer);
    assert_eq!(merged.open(|r| *r), 5);
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
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 9), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();
    let copied = element
        .transfer_into::<RegionHandleFamily<ShapeProfile>, ValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            // The product is an owned `u32` — it borrows nothing, so the predicate releases every
            // member and the fold pins nothing on the source side.
            |_product, _region| false,
            |value, _handle, _brand| *value,
        );
    drop(element);
    drop(producer);
    assert!(
        weak.upgrade().is_none(),
        "an unclaimed producer is released with its envelope, never minted"
    );
    assert_eq!(copied.open(|v| v), 9);
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
    let reach_ptr: *const ReachDescription<ShapeFrame> =
        element.open_at().with_reach(|reach| reach as *const _);
    let home_count = Rc::strong_count(&home);
    let content_count = Rc::strong_count(&content);

    let first = element.duplicate();
    let second = element.duplicate();
    for duplicate in [&first, &second] {
        let ptr = duplicate.open_at().with_reach(|reach| reach as *const _);
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
    let (minted, bundle) = ReachDescription::mint(RegionHandle::from_owner(&*a), &[&source]);
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
    );
    let sealed: Sealed<RefValFamily, Carrier<ShapeFrame>> = Sealed::seal(Witnessed::from_erased(
        Erased::erase(value),
        Carrier::new(reach),
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

/// **`Delivered::open_adopted`** — the adoption mints the value's reach into `dest` and retains the
/// owned bundle there in the same act, so the resealed carrier rests resident with its liveness
/// carried by `dest`'s region. A value living in its producer is adopted into `dest`; after the
/// producer handle drops, `dest`'s own pin is the sole coverage under which the resident seal reads
/// back. A mint that failed to name the producer, or a retention the mint skipped, is a UAF here.
#[test]
fn adopt_settles_resident_value_into_dest() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 7), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();
    let sealed: Sealed<RefValFamily, Carrier<ShapeFrame>> = element
        .open_adopted(RegionHandle::from_owner(&*dest))
        .reseal();
    drop(element);
    drop(producer);
    let pins = StepCoverage::of(Rc::clone(&dest));
    assert_eq!(sealed.open_with(&pins, |r| *r), 7);
}

/// **The region's union bundle** — a region retains ONE deduped [`PinBundle`], folded through
/// [`PinBundle::absorb`], never a bundle per retention: adopting the same producer twice costs one
/// `Rc` in total, and retaining an ancestor of an already-retained member costs none (the member's
/// own `outer` chain already pins it). What survives is an antichain of the deepest owners, and it
/// is the sole pin under the read once every producer handle drops — a fold that dropped the wrong
/// member is a use-after-free here, and the counts gate the refcount a bundle-per-retention list
/// would hold.
#[test]
fn region_retention_folds_into_one_deduped_bundle() {
    let outer = frame();
    let producer = RegionHost::fresh(Some(Rc::clone(&outer)));
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 13), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();
    let handle = RegionHandle::from_owner(&*dest);

    // Each adoption mints the producer into `dest` and folds the owned bundle into the region's
    // union; the second finds the region already pinned and drops its clone.
    let adopted: &u32 = element.adopt_into(handle);
    let retained = Rc::strong_count(&producer);
    let _second: &u32 = element.adopt_into(handle);
    assert_eq!(
        Rc::strong_count(&producer),
        retained,
        "the second adoption of the same region dedupes into the union bundle"
    );

    let outer_before = Rc::strong_count(&outer);
    handle.retain_reach(StepCoverage::of(Rc::clone(&outer)));
    assert_eq!(
        Rc::strong_count(&outer),
        outer_before,
        "the producer's chain already pins its ancestor's region, so subsumption drops the newcomer"
    );

    drop(element);
    drop(producer);
    drop(outer);
    assert_eq!(
        *adopted, 13,
        "the region's union bundle is the sole pin left"
    );
}

/// **The three states and the four transform verbs, end to end** — `Delivered → open_adopted →
/// Opened → reseal → Sealed → open_at → Opened → reseal → Sealed → lift → Delivered`, with every
/// intermediate handle dropped before the final read. The value lives in `producer`'s region
/// throughout; the only thing keeping that region alive after the drops is the chain of pins each
/// verb hands to the next — the adoption's own retention into `dest`, then `dest`'s pin — so a verb
/// that loses a member is a use-after-free here and one that gains a phantom member is a leak.
#[test]
fn transform_verb_round_trip_preserves_liveness() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 11), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();

    // Delivered → Sealed: at rest in `dest`'s table, its pins retained by `dest`'s own region.
    let sealed = element
        .open_adopted(RegionHandle::from_owner(&*dest))
        .reseal();
    let pins = StepCoverage::of(Rc::clone(&dest));
    assert!(
        sealed.open_at(&pins).reach_covers(producer.region()),
        "the adoption composed the value's home into dest's description as an ordinary member"
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
        Witnessed::resident_in::<ShapeProfile>(store_val(&dep_frame, 3), &dep_frame),
        Rc::clone(&dep_frame),
        StepCoverage::empty(),
    );
    let own = frame();
    let ctx: StepContext<ShapeFrame> = StepContext::new(Rc::clone(&own));
    let built = ctx.alloc_with::<RefValFamily, RefValFamily, ShapeProfile>(
        &[&dep],
        |_region, views, _token| views[0],
    );
    drop(dep);
    drop(dep_frame);
    assert_eq!(built.open(|r| *r), 3);
}

/// **`open_adopted` — the adopt that stays open at the destination's own lifetime.** It is
/// [`Delivered::adopt`] and [`Delivered::adopt_into`] fused: the mint stores the value's reach in
/// `dest`'s side table and the region retains the owning bundle, so the returned [`Opened`] borrows
/// at `'d` rather than at a pin borrow — and every handle the value's backing came from can go
/// before it is read. The open's witness is the adopted one, so [`Opened::reseal`] reproduces
/// exactly the seal `adopt` would have handed back.
#[test]
fn open_adopted_reads_at_the_destination_lifetime_after_the_producer_drops() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 23), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();

    let opened = element.open_adopted(RegionHandle::from_owner(&*dest));
    assert!(
        opened.reach_covers(producer.region()),
        "the mint composed the value's home into dest's description as an ordinary member",
    );
    drop(element);
    drop(producer);

    assert_eq!(
        *opened.value(),
        23,
        "the region's retained bundle is the sole pin left"
    );
    let resealed = opened.reseal();
    assert_eq!(
        resealed.open_with(&StepCoverage::<ShapeFrame>::of(Rc::clone(&dest)), |r| *r),
        23,
        "reseal returns exactly the adopted seal",
    );
}

/// **`project` re-families in place.** Nothing moves and nothing is minted: the envelope keeps its
/// residence, its coverage and its witness, and the projection borrows *from* the value the
/// envelope already covers. Dropping the producer handles leaves the envelope's own pins as the
/// only thing keeping the pointee alive, so a projection that dropped them is a use-after-free here.
#[test]
fn project_refamilies_under_the_envelopes_own_pins() {
    let producer = frame();
    let pair = RegionHandle::from_owner(&*producer).alloc_resident::<ValFamily>(31);
    let second = RegionHandle::from_owner(&*producer).alloc_resident::<ValFamily>(37);
    let element: Delivered<PairVals, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>((pair, second), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );

    let projected: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> =
        element.project::<RefValFamily>(|(left, _right), _token| left);
    drop(producer);
    assert_eq!(projected.open(|r| *r), 31);
}

/// **A region never retains a pin on itself.** [`RegionHandle::retain_reach`] applies the same self
/// rule the mint does, so a caller may hand over a whole coverage — a resting cell's claim, home
/// included — without first asking whether that home happens to be this very region. Folding the
/// self-`Rc` in would close a cycle nothing breaks: the region would keep its own owner alive, and
/// the leak detector would report the whole arena at exit.
///
/// The rule is on the *door*, not on `rest_in`, so it holds for every retention — a resident bind's
/// coverage and a run-teardown rehome as much as a splice install. Both cases below run through the
/// one door; the foreign member proves the strip is targeted, not a blanket refusal to retain.
#[test]
fn retain_reach_never_folds_a_regions_own_pin_into_itself() {
    let dest = frame();
    let foreign = frame();
    let handle = RegionHandle::from_owner(&*dest);

    handle.retain_reach(StepCoverage::of(Rc::clone(&dest)));
    assert_eq!(
        dest.region().retained_reach_len(),
        0,
        "a coverage naming only this region retains nothing — a region owning itself never frees"
    );
    assert_eq!(
        Rc::strong_count(&dest),
        1,
        "and takes no `Rc`, so the caller's handle is still the only owner"
    );

    let mut mixed = StepCoverage::of(Rc::clone(&dest));
    mixed.absorb(StepCoverage::of(Rc::clone(&foreign)));
    handle.retain_reach(mixed);
    assert_eq!(
        dest.region().retained_reach_len(),
        1,
        "the strip is targeted: the foreign member is retained, this region alone is dropped"
    );
}

/// The resting cell an embedder stores inside its own bit-copy value — an expression part holding a
/// resolved sub-result. Deriving `Copy` here is the whole point: it compiles only because the seal
/// is `Copy`, and the assert below pins that the part carries no `Drop` glue, so region death runs
/// nothing per cell.
#[derive(Clone, Copy)]
struct RestingPart(Sealed<RefValFamily, Carrier<ShapeFrame>>);

const _: () = assert!(
    !std::mem::needs_drop::<RestingPart>(),
    "a resting cell is pure data — its pins live one level down, in the region"
);

/// **`rest_in` — the drop to rest, with the pins lodged one level down.** The envelope's whole
/// coverage (its value's home among the members) goes into the destination region's union bundle,
/// and what comes back is a bit-copy cell owning nothing. Every handle the value's backing came
/// from is dropped before the read, so the retained bundle is the only pin left: a `rest_in` that
/// lodged nothing is a use-after-free here. Fanning a second cell out of the same envelope costs no
/// extra `Rc` — the union dedupes the region it already pins.
#[test]
fn rest_in_lodges_coverage_for_the_destination_regions_life() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 41), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();

    let cell: Sealed<RefValFamily, Carrier<ShapeFrame>> =
        element.rest_in(RegionHandle::from_owner(&*dest));
    let retained = Rc::strong_count(&producer);
    let twin = element.rest_in(RegionHandle::from_owner(&*dest));
    assert_eq!(
        Rc::strong_count(&producer),
        retained,
        "a second resting cell over the same region dedupes into the union bundle"
    );

    drop(element);
    drop(producer);

    let pin = StepCoverage::<ShapeFrame>::of(Rc::clone(&dest));
    assert_eq!(
        cell.open_with(&pin, |r| *r),
        41,
        "the region's retained bundle is the sole pin left"
    );
    assert_eq!(twin.open_with(&pin, |r| *r), 41);
}

/// **A resting cell travels as plain data.** Copying it is a bit-copy of the erased value and the
/// reference-only carrier — no refcount traffic, no clone — so an embedder duplicates it into as
/// many parts as it likes without touching the ownership tier. Each copy still reads under the
/// region's own hold, after every handle the backing came from is gone.
#[test]
fn a_resting_cell_copies_as_plain_data() {
    let producer = frame();
    let element: Delivered<RefValFamily, Carrier<ShapeFrame>, ShapeFrame> = Delivered::seal(
        Witnessed::resident_in::<ShapeProfile>(store_val(&producer, 47), &producer),
        Rc::clone(&producer),
        StepCoverage::empty(),
    );
    let dest = frame();
    let part = RestingPart(element.rest_in(RegionHandle::from_owner(&*dest)));

    let before = Rc::strong_count(&producer);
    let copies = [part, part, part];
    assert_eq!(
        Rc::strong_count(&producer),
        before,
        "copying a resting cell moves no ownership — the pins stay in the region"
    );

    drop(element);
    drop(producer);

    let pin = StepCoverage::<ShapeFrame>::of(Rc::clone(&dest));
    for copy in copies {
        assert_eq!(copy.0.open_with(&pin, |r| *r), 47);
    }
}

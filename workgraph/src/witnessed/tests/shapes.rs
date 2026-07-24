//! Miri slate (tree borrows) for the abstract carrier shapes the witnessed substrate admits —
//! the reference-only [`Carrier`]'s two liveness channels (residence and reach), the
//! `Residence` × `borrows_host` materialization matrix, envelope duplication, the
//! [`ReachDescription::mint`] home-omission rule, and the [`StepContext::alloc_with`] finish-surface
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
    // `content` is foreign to `host`, so it enters the minted description through the
    // materialize-hosts arm; the returned bundle is the value's owned foreign reach, threaded into
    // the envelope at `seal` so each holder owns its pins.
    let (reach, bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&**host),
        &[],
        &[Rc::clone(content)],
        |_| false,
    );
    Delivered::seal(
        Witnessed::from_erased(Erased::erase(value), Carrier::new(false, reach)),
        Rc::clone(host),
        bundle,
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
        PinBundle::empty(),
    );
    let dest = frame();
    let (merged, _bundle): (Witnessed<RefValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
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
    let (acc1, acc1_bundle) = reach_element(&dest, &content_a, 1)
        .transfer_into::<PairAcc, PairAcc, ShapeProfile>(
            acc0,
            &PinBundle::empty(),
            Residence::Kept,
            |value, (handle, mut values), _brand| {
                values.push(value);
                (handle, values)
            },
        );
    let (acc2, _acc2_bundle) = reach_element(&dest, &content_b, 2)
        .transfer_into::<PairAcc, PairAcc, ShapeProfile>(
            acc1,
            &acc1_bundle,
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
        PinBundle::empty(),
    );
    let dest = frame();
    let (merged, _bundle): (Witnessed<RefValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, RefValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
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
        PinBundle::empty(),
    );
    let dest = frame();
    let (copied, _bundle): (Witnessed<ValFamily, Carrier<ShapeFrame>>, _) =
        element.transfer_into::<RegionHandleFamily<ShapeProfile>, ValFamily, ShapeProfile>(
            dest_handle_acc(&dest),
            &PinBundle::empty(),
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

/// **Envelope duplication shares the description, clones the owned pins** — duplicating for another
/// consumer bit-copies the reference-only carrier, so the reach **description** rides by reference
/// (never re-minted); but the envelope owns its liveness now, so each duplicate clones one retained
/// host `Rc` **and** its owned foreign [`PinBundle`] (one `Rc` per foreign member), giving every
/// fan-out consumer its own pins for the parked period. A re-mint of the description here is the
/// regression this gates; the leak detector is the backstop.
#[test]
fn duplicate_shares_reach_and_clones_owned_pins() {
    let home = frame();
    let content = frame();
    let element = reach_element(&home, &content, 4);
    let reach_ptr: *const ReachDescription<ShapeFrame> =
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
        assert_eq!(ptr, reach_ptr, "the reach description rides by reference");
    }
    assert_eq!(
        Rc::strong_count(&home),
        home_count + 2,
        "one retained-host clone per duplicate"
    );
    assert_eq!(
        Rc::strong_count(&content),
        content_count + 2,
        "one owned foreign-pin clone per duplicate — each consumer owns its pins"
    );
}

/// **`ReachDescription::mint` home-omission** — a description hosted in region A never names `A`
/// (the self-cycle rule): minting materialize-hosts that include the destination's own frame keeps
/// only the foreign member, and the minted **bundle** is that member's owner (the description's
/// mirror holds only a `Weak`, so A's side table pins nothing). Dropping the bundle releases the
/// member; dropping A frees A with no self-cycle — the Miri leak audit over this test signs off the
/// no-self-cycle shape.
#[test]
fn mint_home_omission_prevents_self_cycle() {
    let a = frame();
    let b = frame();
    let weak_a = Rc::downgrade(&a);
    let weak_b = Rc::downgrade(&b);

    let (minted, bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&*a),
        &[],
        &[Rc::clone(&a), Rc::clone(&b)],
        |_| false,
    );
    let minted = minted.expect("the foreign member materializes");
    assert!(
        matches!(minted.members().as_slice(), [only] if Rc::ptr_eq(only, &b)),
        "home is omitted; the foreign member is kept"
    );

    drop(b);
    assert!(
        weak_b.upgrade().is_some(),
        "the minted bundle holds the sole surviving member (A's side table only names it)"
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

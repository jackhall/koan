//! The reach side table's **interning** slate: [`Region::intern_reach_retained`] get-or-mints keyed
//! on the canonical member set, so one description exists per distinct reach per region, pointer
//! identity over descriptions is member-set equality within a region, and an entry's existence is
//! proof the region already retains what it names. Runs over a library-only
//! profile (`RegionHost` frames, no storage families at all) — no embedder type. Counts are read as
//! deltas off the thread-local [`RegionMetrics`], which test threads see in isolation, so each test
//! measures its own mint traffic.

use std::rc::Rc;

use super::super::*;

/// The library-only profile the intern slate runs over. A region's reach table is independent of
/// its family storage, so the family list is empty.
struct InternProfile;

impl StorageProfile for InternProfile {
    type Families = ();
    type FrameOwner = RegionHost<InternProfile>;
}

type InternFrame = RegionHost<InternProfile>;

/// A fresh per-call frame with no ancestor — a member no other member subsumes.
fn frame() -> Rc<InternFrame> {
    RegionHost::fresh(None)
}

/// A fresh per-call frame chained under `outer`, so holding it pins `outer`'s region too — the
/// subsumption relation [`PinBundle::insert`] normalizes against.
fn inner(outer: &Rc<InternFrame>) -> Rc<InternFrame> {
    RegionHost::fresh(Some(Rc::clone(outer)))
}

/// The reach counters' movement across `body`, paired with whatever `body` returns. A delta rather
/// than a [`reset_region_metrics`] window: a reset also zeroes the live-region gauge, which the
/// frames a test already holds would then underflow when they drop.
fn reach_delta<R>(body: impl FnOnce() -> R) -> (R, RegionMetrics) {
    let before = region_metrics();
    let result = body();
    let after = region_metrics();
    (
        result,
        RegionMetrics {
            reach_interned: after.reach_interned - before.reach_interned,
            reach_intern_hits: after.reach_intern_hits - before.reach_intern_hits,
            reach_retention_folds: after.reach_retention_folds - before.reach_retention_folds,
            ..RegionMetrics::default()
        },
    )
}

/// Mint into `home`'s region over one source bundle per owner in `members`.
fn mint_over<'a>(
    home: &'a Rc<InternFrame>,
    members: &[&Rc<InternFrame>],
) -> &'a ReachDescription<InternFrame> {
    let bundles: Vec<PinBundle<InternFrame>> = members
        .iter()
        .map(|m| PinBundle::singleton(Rc::clone(m)))
        .collect();
    let sources: Vec<&PinBundle<InternFrame>> = bundles.iter().collect();
    ReachDescription::mint_resident(RegionHandle::from_owner(&**home), &sources)
}

/// A miss allocates: two distinct member sets get two distinct entries.
#[test]
fn miss_allocates_per_distinct_member_set() {
    let home = frame();
    let (a, b) = (frame(), frame());

    let ((first, second), metrics) =
        reach_delta(|| (mint_over(&home, &[&a]), mint_over(&home, &[&b])));

    assert!(!std::ptr::eq(first, second));
    assert_eq!(metrics.reach_interned, 2);
    assert_eq!(metrics.reach_intern_hits, 0);
}

/// A hit returns the existing entry: the same member set twice is one description.
#[test]
fn hit_returns_the_existing_entry() {
    let home = frame();
    let a = frame();

    let ((first, second), metrics) =
        reach_delta(|| (mint_over(&home, &[&a]), mint_over(&home, &[&a])));

    assert!(std::ptr::eq(first, second));
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 1);
}

/// The key is canonical: the same members composed in opposite source order intern to one entry,
/// because [`PinBundle::intern_key`] sorts the antichain's addresses.
#[test]
fn key_is_order_independent() {
    let home = frame();
    let (a, b) = (frame(), frame());

    let ((forward, reverse), metrics) =
        reach_delta(|| (mint_over(&home, &[&a, &b]), mint_over(&home, &[&b, &a])));

    assert!(std::ptr::eq(forward, reverse));
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 1);
}

/// Subsumption feeds the key: a bundle naming an owner an outer member already pins keys the same
/// as the bundle without it, because the antichain is normalized before the key is taken.
#[test]
fn subsumed_member_does_not_change_the_key() {
    let home = frame();
    let outer = frame();
    let deep = inner(&outer);
    // Mint `outer`'s region so `deep.pins_region(outer.region())` has something to match.
    let _ = outer.region();

    let ((with_outer, deep_only), metrics) = reach_delta(|| {
        (
            mint_over(&home, &[&deep, &outer]),
            mint_over(&home, &[&deep]),
        )
    });

    assert!(std::ptr::eq(with_outer, deep_only));
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 1);
}

/// On an intern hit the region-lifetime retention fold is skipped: minting and retaining the same
/// reach twice costs one description and one fold, and leaves the union bundle unchanged.
#[test]
fn hit_skips_the_retention_fold() {
    let home = frame();
    let a = frame();
    let handle = RegionHandle::from_owner(&*home);

    let ((first, after_first, second), metrics) = reach_delta(|| {
        let first = handle.mint_retained(&[&StepCoverage::of(Rc::clone(&a))]);
        let after_first = home.region().retained_reach_len();
        let second = handle.mint_retained(&[&StepCoverage::of(Rc::clone(&a))]);
        (first, after_first, second)
    });

    assert!(std::ptr::eq(first, second));
    assert_eq!(after_first, 1);
    assert_eq!(home.region().retained_reach_len(), after_first);
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 1);
    assert_eq!(metrics.reach_retention_folds, 1);
}

/// **A hit is proof the destination already pins.** The threaded door (the composition engine's,
/// whose product also travels under transit pins of its own) mints first; its transit bundle is
/// dropped, and a later `mint_retained` over the same members hits that entry and folds nothing.
/// The member frame's only remaining owner is then the region's union bundle — so if a hit were
/// *not* proof, dropping every handle here would free the member and every read through the
/// interned description would dangle.
#[test]
fn hit_is_proof_the_region_already_pins() {
    let home = frame();
    let a = frame();
    let weak_a = Rc::downgrade(&a);
    let handle = RegionHandle::from_owner(&*home);

    let (composed, transit) =
        ReachDescription::mint_resident_threaded(handle, &[&PinBundle::singleton(Rc::clone(&a))]);
    drop(transit);

    let (hit, metrics) = reach_delta(|| handle.mint_retained(&[&StepCoverage::of(Rc::clone(&a))]));
    assert!(std::ptr::eq(composed, hit));
    assert_eq!(metrics.reach_intern_hits, 1);
    assert_eq!(metrics.reach_retention_folds, 0);

    drop(a);
    assert_eq!(
        home.region().retained_reach_len(),
        1,
        "the first mint's own retention is what still pins the member"
    );
    assert!(
        weak_a.upgrade().is_some(),
        "an intern hit skipped the fold because the region already held the pin"
    );
    assert!(hit.pins_region(
        weak_a
            .upgrade()
            .expect("the region's retention keeps the member alive")
            .region()
    ));
}

/// The empty description is a **per-region** interned singleton: every region-pure value in one
/// region shares its entry, and a second region has its own.
#[test]
fn empty_description_is_a_per_region_singleton() {
    let home = frame();
    let other = frame();

    let ((first, second, elsewhere), metrics) = reach_delta(|| {
        (
            mint_over(&home, &[]),
            mint_over(&home, &[]),
            mint_over(&other, &[]),
        )
    });

    assert!(first.is_empty());
    assert!(std::ptr::eq(first, second));
    assert!(!std::ptr::eq(first, elsewhere));
    assert_eq!(metrics.reach_interned, 2);
    assert_eq!(metrics.reach_intern_hits, 1);
}

/// A mint whose only member is the destination's own region interns under that member: the key
/// comes off the composed antichain, *before* the self-rule strip, so it matches the stored
/// description's membership rather than the bundle handed back to the caller. Were it taken after,
/// a home-borrowing value and a region-pure one would collide on the empty key.
#[test]
fn key_precedes_the_self_rule_strip() {
    let home = frame();

    let ((borrows_home, region_pure), metrics) =
        reach_delta(|| (mint_over(&home, &[&home]), mint_over(&home, &[])));

    assert!(borrows_home.borrows_home());
    assert!(region_pure.is_empty());
    assert!(!std::ptr::eq(borrows_home, region_pure));
    assert_eq!(metrics.reach_interned, 2);
    assert_eq!(metrics.reach_intern_hits, 0);
}

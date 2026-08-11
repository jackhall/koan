//! The **pin-ring** slate: [`Region::retain_reach`]'s debug-mode cycle detector, which reports the
//! over-pinning direction every other residence audit passes silently. A mutual pin — region A's
//! union bundle retaining owner B while region B's retains owner A — is expressible in safe code,
//! defeats the refcount-driven region free, and is unreachable from any live root the moment the
//! external references drop, which is why detection runs online at the fold that closes the ring.
//!
//! Runs over a library-only profile (`RegionHost` frames, no storage families), so what is measured
//! is the retention graph alone. Each test resets the thread-local log first; test threads see it in
//! isolation.

use std::rc::Rc;

use super::super::*;

/// The library-only profile the ring slate runs over — reach retention is independent of family
/// storage, so there is none.
struct CycleProfile;

impl StorageProfile for CycleProfile {
    type FrameOwner = RegionHost<CycleProfile>;
}

type CycleFrame = RegionHost<CycleProfile>;

/// A fresh per-call frame, optionally chained under `outer` — the chain edge the detector expands
/// through [`PinsRegion::for_each_pinned_region`].
fn frame(outer: Option<Rc<CycleFrame>>) -> Rc<CycleFrame> {
    RegionHost::fresh(outer)
}

/// The owner identity a report names.
fn identity(owner: &Rc<CycleFrame>) -> usize {
    Rc::as_ptr(owner) as usize
}

/// Retain `member`'s pin for `holder`'s region's whole life — the fold the detector runs at.
fn retain(holder: &Rc<CycleFrame>, member: &Rc<CycleFrame>) {
    holder
        .region()
        .retain_reach(PinBundle::singleton(Rc::clone(member)));
}

/// Dismantle a ring the test built on purpose. A reported ring is a **real** leak — that is the
/// whole point of detecting it — so every host in one stays allocated to process exit and the Miri
/// leak audit reports it. A test that constructs a ring owes this teardown; one that constructs an
/// acyclic graph does not, because ordinary `Drop` reclaims it.
fn dismantle(frames: &[&Rc<CycleFrame>]) {
    for frame in frames {
        frame.region().release_retained_for_test();
    }
}

/// The acceptance case: a mutual pin between two per-call regions is reported, and the one-way half
/// of it is not. Blame names the region that closed the ring, and the path names the owners walked
/// from the newly retained member back to it.
#[test]
fn mutual_pin_is_reported() {
    reset_pin_cycle_reports();
    let a = frame(None);
    let b = frame(None);

    // Half a ring: A retains B. B pins only its own region, which A's retention does not name.
    retain(&a, &b);
    assert!(
        pin_cycle_reports().is_empty(),
        "one-way retention is no ring"
    );

    // The closing edge: B retains A, whose region's bundle holds B, which pins B's own region.
    retain(&b, &a);
    let reports = pin_cycle_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].retainer, identity(&b));
    assert_eq!(reports[0].path, vec![identity(&a), identity(&b)]);

    dismantle(&[&a, &b]);
}

/// The ring closes through a **chain** edge rather than a retention one: A retains a child of B, and
/// holding that child pins B's region through its `outer` link. Exercises the ancestor walk in
/// [`PinsRegion::for_each_pinned_region`], which a `pins_region`-only detector would miss.
#[test]
fn chain_mediated_ring_is_reported() {
    reset_pin_cycle_reports();
    let a = frame(None);
    let b = frame(None);
    let child_of_b = frame(Some(Rc::clone(&b)));

    retain(&a, &child_of_b);
    assert!(pin_cycle_reports().is_empty());

    retain(&b, &a);
    let reports = pin_cycle_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].retainer, identity(&b));
    assert_eq!(
        reports[0].path,
        vec![identity(&a), identity(&child_of_b)],
        "the walk reaches B's region through the child's outer chain"
    );

    dismantle(&[&a, &b]);
}

/// The negative: a retention *chain* — A retains B, B retains C, C retains a leaf whose own ancestor
/// chain is three regions deep — forms no ring, so the walk runs its full depth and reports nothing.
#[test]
fn acyclic_retention_reports_nothing() {
    reset_pin_cycle_reports();
    let root = frame(None);
    let middle = frame(Some(Rc::clone(&root)));
    let leaf = frame(Some(Rc::clone(&middle)));
    let (a, b, c) = (frame(None), frame(None), frame(None));

    // Mint every region on the leaf's chain, so the chain edges the walk expands are real.
    let _ = root.region();
    let _ = middle.region();
    let _ = leaf.region();

    // Built innermost-first, so the last fold walks the whole chain: b → c → leaf → its ancestors.
    retain(&c, &leaf);
    retain(&b, &c);
    retain(&a, &b);

    assert!(pin_cycle_reports().is_empty());
}

/// The eternal rule cuts the ring before it forms: an eternal host's storage outlives every region
/// that could retain it, so no pin on it is ever taken and there is nothing to report — even though
/// the eternal region's own bundle does hold the per-call owner.
#[test]
fn eternal_member_reports_nothing() {
    reset_pin_cycle_reports();
    let eternal = RegionHost::<CycleProfile>::fresh_eternal();
    let per_call = frame(None);

    retain(&eternal, &per_call);
    retain(&per_call, &eternal);

    assert!(pin_cycle_reports().is_empty());
    assert_eq!(
        per_call.region().retained_reach_len(),
        0,
        "the eternal member never entered the retention at all"
    );
}

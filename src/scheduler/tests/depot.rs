//! The build depot alone: handles, held counts, and eviction by bytes, least recently used unheld
//! leaf first.

use std::num::NonZeroUsize;

use smallvec::SmallVec;

use crate::scheduler::depot::{BuildDepot, Composed, Settled, Under};
use crate::scope::{Binding, BuildLayer, Composition, Facts, LoadFacts, ShapeError};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{BuildBudget, DepotRef, LayerKey};

/// The key a test names by `at`.
fn key(at: usize) -> LayerKey {
    LayerKey(DigestHasher::new(Tag::Number).count(at).finished())
}

/// A settlement holding no fact, over no parent's.
fn bare() -> Settled<'static> {
    Settled::build(
        Under::Root,
        SmallVec::new(),
        &LoadFacts::EMPTY,
        |_, below: Facts<'_>, _, _| Some(BuildLayer::empty(below.top())),
    )
    .expect("a bare settlement settles")
}

/// What one settlement weighs: every settlement here holds no fact, so all weigh the same.
fn one() -> usize {
    let mut depot = BuildDepot::new(BuildBudget::default());
    let at = depot.insert(DepotRef::ROOT, key(0), bare(), SmallVec::new());
    depot.bytes_of(at).expect("a new settlement is live")
}

/// A depot whose unheld settlements stay within `count` settlements' bytes.
fn depot_of(count: usize) -> BuildDepot<'static> {
    let bytes = NonZeroUsize::new(count * one()).expect("a settlement weighs something");
    BuildDepot::new(BuildBudget { bytes })
}

/// Settle `key` under `parent`, borrowed for the frame being born, then evict to the budget, as a
/// miss does; and release the borrow, as the frame's end does.
fn missed(depot: &mut BuildDepot<'static>, parent: DepotRef, at: usize) -> DepotRef {
    let settled = depot.insert(parent, key(at), bare(), SmallVec::new());
    depot.borrow(settled);
    depot.evict_to_budget();
    depot.release(settled);
    settled
}

#[test]
fn a_stale_handle_names_nothing_once_its_slot_is_reused() {
    let mut depot = depot_of(1);
    let first = missed(&mut depot, DepotRef::ROOT, 1);
    missed(&mut depot, DepotRef::ROOT, 2);
    // A third miss evicts the first, and the fourth settlement takes its slot.
    missed(&mut depot, DepotRef::ROOT, 3);
    assert!(!depot.is_live(first));
    let fourth = missed(&mut depot, DepotRef::ROOT, 4);
    assert!(depot.is_live(fourth));
    assert_eq!(
        first.opened().map(|(slot, _)| slot),
        fourth.opened().map(|(slot, _)| slot),
        "the freed slot is reused"
    );
    assert_ne!(first, fourth, "under a new generation");
    assert_eq!(
        depot.find(DepotRef::ROOT, key(1)),
        None,
        "its key names nothing"
    );
    assert_eq!(depot.bytes_of(first), None);
    assert!(depot.is_live(DepotRef::ROOT), "the root is always live");
}

#[test]
fn eviction_takes_the_least_recently_used_unheld_leaf_first() {
    let mut depot = depot_of(2);
    let one = missed(&mut depot, DepotRef::ROOT, 1);
    let two = missed(&mut depot, DepotRef::ROOT, 2);
    let three = missed(&mut depot, DepotRef::ROOT, 3);
    // A hit on `one` makes `two` the least recently used.
    assert_eq!(depot.find(DepotRef::ROOT, key(1)), Some(one));
    depot.stamp(one);
    let four = missed(&mut depot, DepotRef::ROOT, 4);
    assert!(!depot.is_live(two), "`two` goes first");
    assert!([one, three, four].iter().all(|at| depot.is_live(*at)));
    let five = missed(&mut depot, DepotRef::ROOT, 5);
    assert!(
        !depot.is_live(three),
        "then `three`, used before `one`'s hit"
    );
    assert!([one, four, five].iter().all(|at| depot.is_live(*at)));
    assert_eq!(depot.len(), 3);
}

#[test]
fn a_borrowed_settlement_and_every_ancestor_of_it_stay_through_eviction() {
    let mut depot = depot_of(1);
    let outer = depot.insert(DepotRef::ROOT, key(1), bare(), SmallVec::new());
    let inner = depot.insert(outer, key(2), bare(), SmallVec::new());
    let leaf = depot.insert(inner, key(3), bare(), SmallVec::new());
    depot.borrow(leaf);
    assert_eq!(depot.unheld_bytes(), 0, "a borrow holds its every ancestor");
    for at in 4..8 {
        missed(&mut depot, DepotRef::ROOT, at);
    }
    assert!([outer, inner, leaf].iter().all(|at| depot.is_live(*at)));
    assert_eq!(depot.borrowers(leaf), Some(1));
    assert_eq!(
        depot.borrowers(inner),
        Some(0),
        "an ancestor is held, not borrowed"
    );
    depot.release(leaf);
    missed(&mut depot, DepotRef::ROOT, 8);
    assert!(
        [outer, inner, leaf].iter().all(|at| !depot.is_live(*at)),
        "released, the chain goes leaf first"
    );
}

#[test]
fn unheld_bytes_stay_within_the_budget_after_every_miss() {
    let budget = 3 * one();
    let mut depot = depot_of(3);
    let mut held = Vec::new();
    for at in 0..40 {
        let parent = match at % 3 {
            0 => DepotRef::ROOT,
            _ => *held.last().unwrap_or(&DepotRef::ROOT),
        };
        let parent = if depot.is_live(parent) {
            parent
        } else {
            DepotRef::ROOT
        };
        let settled = depot.insert(parent, key(at), bare(), SmallVec::new());
        depot.borrow(settled);
        depot.evict_to_budget();
        assert!(depot.unheld_bytes() <= budget, "after miss {at}");
        // Every fifth settlement stays borrowed; the rest end at once.
        if at % 5 == 0 {
            held.push(settled);
        } else {
            depot.release(settled);
        }
    }
    assert!(
        held.iter().all(|at| depot.is_live(*at)),
        "a borrowed settlement stays"
    );
    for at in held.drain(..) {
        depot.release(at);
    }
    assert_eq!(depot.unheld_bytes(), depot.len() * one());
}

#[test]
fn a_parent_is_evicted_only_after_its_last_child() {
    let mut depot = depot_of(1);
    let parent = missed(&mut depot, DepotRef::ROOT, 1);
    let child = missed(&mut depot, parent, 2);
    // `parent` is the least recently used, but it has a child, so `child` goes first.
    let other = missed(&mut depot, DepotRef::ROOT, 3);
    assert!(depot.is_live(parent) && !depot.is_live(child));
    assert!(depot.is_live(other));
    // Childless now, `parent` goes at the next miss.
    missed(&mut depot, DepotRef::ROOT, 4);
    assert!(!depot.is_live(parent));
}

#[test]
fn a_child_settlement_keeps_its_parent_and_releases_it_last() {
    let parent = std::rc::Rc::new(bare());
    let child = Settled::build(
        Under::Settlement(std::rc::Rc::clone(&parent)),
        SmallVec::new(),
        &LoadFacts::EMPTY,
        |_, below: Facts<'_>, _, _| Some(BuildLayer::empty(below.top())),
    )
    .expect("a bare settlement settles");
    // The depot's handle on the parent goes first, as an eviction of it would: the child's
    // facts still read through to the parent's, which its owner alone keeps, and dropping the
    // child releases the parent last.
    drop(parent);
    let mut layers = 0;
    Facts::under(&LoadFacts::EMPTY, Some(child.facts())).each_term(&mut |_| layers += 1);
    assert_eq!(layers, 0, "a bare settlement holds no term");
    drop(child);
}

#[test]
fn a_pinned_composition_outlives_its_last_pinner() {
    // A composition whose facts hold a refusal laid in its own bump.
    let at = crate::tests::nowhere();
    let composition = std::rc::Rc::new(Composed::build(
        crate::tests::composed_syntax(),
        |syntax, bump, _| {
            let refusal = &*bump.alloc(ShapeError::NoKeyword { at });
            let layer = &*bump.alloc(BuildLayer::EMPTY.refusing(Some(refusal)));
            Composition::new(
                syntax.syntax(),
                bump.alloc(LoadFacts::EMPTY.rebased(layer)),
                &[],
            )
        },
    ));
    // A settlement pinning it, whose facts read that refusal out of the composition's bump.
    let pinner = Settled::build(
        Under::Root,
        SmallVec::from_iter([std::rc::Rc::clone(&composition)]),
        &LoadFacts::EMPTY,
        |bump, _, pinned, _| {
            let refusal = pinned[0]
                .refusal()
                .expect("the composition holds a refusal");
            Some(BuildLayer::refused(bump, None, Binding::EMPTY, refusal))
        },
    )
    .expect("a pinner settles");
    // The depot's handle on the composition goes first; the pinner still reads it, and releases
    // it last.
    drop(composition);
    assert!(matches!(
        pinner.facts().refusal(),
        Some(ShapeError::NoKeyword { at: held }) if *held == at
    ));
    drop(pinner);
}

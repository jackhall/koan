//! The build arena alone: handles, held counts, and eviction by bytes, least recently used unheld
//! leaf first.

use std::num::NonZeroUsize;

use smallvec::SmallVec;

use crate::scheduler::arena::{BuildArena, Settled, Under};
use crate::scope::{Facts, LoadFacts, Settlement};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ArenaRef, BuildBudget, SettlementKey};

/// The key a test names by `at`.
fn key(at: usize) -> SettlementKey {
    SettlementKey(DigestHasher::new(Tag::Number).count(at).finished())
}

/// A settlement holding no fact, over no parent's.
fn bare() -> Settled<'static> {
    Settled::build(
        Under::Root,
        SmallVec::new(),
        &LoadFacts::EMPTY,
        |_, below: Facts<'_>, _, _| Some(Settlement::empty(below.top())),
    )
    .expect("a bare settlement settles")
}

/// What one settlement weighs: every settlement here holds no fact, so all weigh the same.
fn one() -> usize {
    let mut arena = BuildArena::new(BuildBudget::default());
    let at = arena.insert(ArenaRef::ROOT, key(0), bare(), SmallVec::new());
    arena.bytes_of(at).expect("a new settlement is live")
}

/// An arena whose unheld settlements stay within `count` settlements' bytes.
fn arena_of(count: usize) -> BuildArena<'static> {
    let bytes = NonZeroUsize::new(count * one()).expect("a settlement weighs something");
    BuildArena::new(BuildBudget { bytes })
}

/// Settle `key` under `parent`, borrowed for the frame being born, then evict to the budget, as a
/// miss does; and release the borrow, as the frame's end does.
fn missed(arena: &mut BuildArena<'static>, parent: ArenaRef, at: usize) -> ArenaRef {
    let settled = arena.insert(parent, key(at), bare(), SmallVec::new());
    arena.borrow(settled);
    arena.evict_to_budget();
    arena.release(settled);
    settled
}

#[test]
fn a_stale_handle_names_nothing_once_its_slot_is_reused() {
    let mut arena = arena_of(1);
    let first = missed(&mut arena, ArenaRef::ROOT, 1);
    missed(&mut arena, ArenaRef::ROOT, 2);
    // A third miss evicts the first, and the fourth settlement takes its slot.
    missed(&mut arena, ArenaRef::ROOT, 3);
    assert!(!arena.is_live(first));
    let fourth = missed(&mut arena, ArenaRef::ROOT, 4);
    assert!(arena.is_live(fourth));
    assert_eq!(
        first.opened().map(|(slot, _)| slot),
        fourth.opened().map(|(slot, _)| slot),
        "the freed slot is reused"
    );
    assert_ne!(first, fourth, "under a new generation");
    assert_eq!(
        arena.find(ArenaRef::ROOT, key(1)),
        None,
        "its key names nothing"
    );
    assert_eq!(arena.bytes_of(first), None);
    assert!(arena.is_live(ArenaRef::ROOT), "the root is always live");
}

#[test]
fn eviction_takes_the_least_recently_used_unheld_leaf_first() {
    let mut arena = arena_of(2);
    let one = missed(&mut arena, ArenaRef::ROOT, 1);
    let two = missed(&mut arena, ArenaRef::ROOT, 2);
    let three = missed(&mut arena, ArenaRef::ROOT, 3);
    // A hit on `one` makes `two` the least recently used.
    assert_eq!(arena.find(ArenaRef::ROOT, key(1)), Some(one));
    arena.stamp(one);
    let four = missed(&mut arena, ArenaRef::ROOT, 4);
    assert!(!arena.is_live(two), "`two` goes first");
    assert!([one, three, four].iter().all(|at| arena.is_live(*at)));
    let five = missed(&mut arena, ArenaRef::ROOT, 5);
    assert!(
        !arena.is_live(three),
        "then `three`, used before `one`'s hit"
    );
    assert!([one, four, five].iter().all(|at| arena.is_live(*at)));
    assert_eq!(arena.len(), 3);
}

#[test]
fn a_borrowed_settlement_and_every_ancestor_of_it_stay_through_eviction() {
    let mut arena = arena_of(1);
    let outer = arena.insert(ArenaRef::ROOT, key(1), bare(), SmallVec::new());
    let inner = arena.insert(outer, key(2), bare(), SmallVec::new());
    let leaf = arena.insert(inner, key(3), bare(), SmallVec::new());
    arena.borrow(leaf);
    assert_eq!(arena.unheld_bytes(), 0, "a borrow holds its every ancestor");
    for at in 4..8 {
        missed(&mut arena, ArenaRef::ROOT, at);
    }
    assert!([outer, inner, leaf].iter().all(|at| arena.is_live(*at)));
    assert_eq!(arena.borrowers(leaf), Some(1));
    assert_eq!(
        arena.borrowers(inner),
        Some(0),
        "an ancestor is held, not borrowed"
    );
    arena.release(leaf);
    missed(&mut arena, ArenaRef::ROOT, 8);
    assert!(
        [outer, inner, leaf].iter().all(|at| !arena.is_live(*at)),
        "released, the chain goes leaf first"
    );
}

#[test]
fn unheld_bytes_stay_within_the_budget_after_every_miss() {
    let budget = 3 * one();
    let mut arena = arena_of(3);
    let mut held = Vec::new();
    for at in 0..40 {
        let parent = match at % 3 {
            0 => ArenaRef::ROOT,
            _ => *held.last().unwrap_or(&ArenaRef::ROOT),
        };
        let parent = if arena.is_live(parent) {
            parent
        } else {
            ArenaRef::ROOT
        };
        let settled = arena.insert(parent, key(at), bare(), SmallVec::new());
        arena.borrow(settled);
        arena.evict_to_budget();
        assert!(arena.unheld_bytes() <= budget, "after miss {at}");
        // Every fifth settlement stays borrowed; the rest end at once.
        if at % 5 == 0 {
            held.push(settled);
        } else {
            arena.release(settled);
        }
    }
    assert!(
        held.iter().all(|at| arena.is_live(*at)),
        "a borrowed settlement stays"
    );
    for at in held.drain(..) {
        arena.release(at);
    }
    assert_eq!(arena.unheld_bytes(), arena.len() * one());
}

#[test]
fn a_parent_is_evicted_only_after_its_last_child() {
    let mut arena = arena_of(1);
    let parent = missed(&mut arena, ArenaRef::ROOT, 1);
    let child = missed(&mut arena, parent, 2);
    // `parent` is the least recently used, but it has a child, so `child` goes first.
    let other = missed(&mut arena, ArenaRef::ROOT, 3);
    assert!(arena.is_live(parent) && !arena.is_live(child));
    assert!(arena.is_live(other));
    // Childless now, `parent` goes at the next miss.
    missed(&mut arena, ArenaRef::ROOT, 4);
    assert!(!arena.is_live(parent));
}

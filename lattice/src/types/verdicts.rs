//! The verdict tables: caches of relation answers each tier keeps beside its nodes.
//!
//! A verdict is keyed by `(subject digest, candidate digest, relation)` ([`VerdictKey`]). A
//! verdict over a digest pair is a pure function — once computed it never changes — so verdicts
//! are never load-bearing: a forgotten one costs a re-walk of the relation, never a wrong answer.
//! That leaves each tier free to choose what it keeps.
//!
//! - The **root** keeps a fixed run of two-slot buckets ([`FixedVerdicts`]), laid once and never
//!   resized, since the root never frees. A full bucket evicts the slot not touched last. While
//!   the load builds the root, its builder lays the run at the first verdict ([`VerdictTable`]),
//!   and the freeze hands the run over.
//! - A **branch** keeps every verdict it is given, in a [`DigestTable`](crate::bump::DigestTable)
//!   of [`VerdictEntry`]s laid in its half at its first verdict and released with it.
//!
//! The chain decides which tier records a verdict ([chain.rs](super::chain)); the relations that
//! read and record verdicts live in [`order`](super::order), [`ranking`](super::ranking) and
//! [`unify`](super::unify).

use std::cell::Cell;

use crate::bump::{BumpAllocator, Keyed};

use super::digest::TypeDigest;

/// Which question a recorded verdict answers. They never alias — each digest domain is disjoint by
/// construction, and a class verdict names its class — but the enum still keys the table
/// explicitly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Relation {
    /// [`is_subtype_of`](super::order::is_subtype_of), the one order.
    Subtype,
    /// [`fits`](super::order::fits), the relation a question reads.
    Fits,
    /// [`member_at_least`](super::unify::member_at_least).
    MemberAtLeast,
    /// [`class_at_least`](super::ranking::class_at_least) at the class it names.
    ClassAtLeast(u16),
}

impl Relation {
    /// The bits a key's fold mixes in: distinct per relation and per class.
    fn bits(self) -> u64 {
        match self {
            Relation::Subtype => 0,
            Relation::Fits => 1,
            Relation::MemberAtLeast => 2,
            Relation::ClassAtLeast(class) => 3 + u64::from(class),
        }
    }
}

/// What a verdict is found by: the two types and the question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerdictKey {
    pub subject: TypeDigest,
    pub candidate: TypeDigest,
    pub relation: Relation,
}

impl VerdictKey {
    /// The key's uniformly distributed word. A digest is already a uniformly distributed hash, so
    /// the two digests' low words fold together — rotated apart, so `(a, b)` and `(b, a)` differ —
    /// with the relation mixed in. Every table compares the whole key, so a fold collision costs a
    /// probe and never a wrong verdict.
    fn fold(self) -> u64 {
        (self.subject.0 as u64).rotate_left(32) ^ (self.candidate.0 as u64) ^ self.relation.bits()
    }
}

/// Slots in the root's verdict table. A power of two, two slots to a bucket.
pub(super) const VERDICT_SLOTS: usize = 1024;

/// One slot of a fixed table: a whole key, its verdict, and the bucket's recency bit.
#[derive(Clone, Copy)]
pub(super) struct VerdictSlot {
    key: VerdictKey,
    verdict: bool,
    occupied: bool,
    /// Set on the slot of its bucket touched last; the other slot is the one an insert evicts.
    recent: bool,
}

impl VerdictSlot {
    const EMPTY: Self = Self {
        key: VerdictKey {
            subject: TypeDigest(0),
            candidate: TypeDigest(0),
            relation: Relation::Subtype,
        },
        verdict: false,
        occupied: false,
        recent: false,
    };

    fn holds(self, key: VerdictKey) -> bool {
        self.occupied && self.key == key
    }
}

/// The root's table: a fixed run of two-slot buckets, each slot copied in and out whole, so it
/// records through a shared borrow of a frozen root. Covariant in `'r`.
pub(super) struct FixedVerdicts<'r> {
    slots: &'r [Cell<VerdictSlot>],
    tally: Tally,
}

impl FixedVerdicts<'_> {
    /// The recorded verdict for `key`, if any. A hit marks its slot the one touched last.
    pub(super) fn get(&self, key: VerdictKey) -> Option<bool> {
        get(self.slots, key)
    }

    /// Record `verdict` for `key`.
    pub(super) fn record(&self, key: VerdictKey, verdict: bool) {
        record(self.slots, &self.tally, key, verdict);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn tally(&self) -> (u64, u64) {
        self.tally.read()
    }
}

/// The root builder's table: empty until the first verdict is recorded, then `len` slots long for
/// good, and handed to the frozen root whole.
pub(super) struct VerdictTable<'r> {
    slots: Cell<&'r [Cell<VerdictSlot>]>,
    len: usize,
    tally: Tally,
}

impl<'r> VerdictTable<'r> {
    /// A table that will hold `len` slots once laid.
    pub(super) fn with_slots(len: usize) -> Self {
        assert!(len.is_power_of_two() && len >= 2);
        Self {
            slots: Cell::new(&[]),
            len,
            tally: Tally::default(),
        }
    }

    /// The recorded verdict for `key`, if any.
    pub(super) fn get(&self, key: VerdictKey) -> Option<bool> {
        let slots = self.slots.get();
        if slots.is_empty() {
            return None;
        }
        get(slots, key)
    }

    /// Record `verdict` for `key`, laying the run in `bump` on the first record.
    pub(super) fn record(&self, bump: BumpAllocator<'r>, key: VerdictKey, verdict: bool) {
        record(self.laid(bump), &self.tally, key, verdict);
    }

    /// The table as the frozen root keeps it, its run laid if no verdict was ever recorded.
    pub(super) fn freeze(&self, bump: BumpAllocator<'r>) -> FixedVerdicts<'r> {
        FixedVerdicts {
            slots: self.laid(bump),
            tally: self.tally.clone(),
        }
    }

    /// The run, laid in `bump` if it is not yet.
    fn laid(&self, bump: BumpAllocator<'r>) -> &'r [Cell<VerdictSlot>] {
        let mut slots = self.slots.get();
        if slots.is_empty() {
            slots = bump.alloc_slice_fill_with(self.len, |_| Cell::new(VerdictSlot::EMPTY));
            self.slots.set(slots);
        }
        slots
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn tally(&self) -> (u64, u64) {
        self.tally.read()
    }
}

/// The two slots of `slots` that `key` lands in.
fn bucket(slots: &[Cell<VerdictSlot>], key: VerdictKey) -> &[Cell<VerdictSlot>] {
    let first = 2 * ((key.fold() as usize) & (slots.len() / 2 - 1));
    &slots[first..first + 2]
}

/// The verdict recorded for `key` in a fixed run, if any. A hit marks its slot the one touched
/// last.
fn get(slots: &[Cell<VerdictSlot>], key: VerdictKey) -> Option<bool> {
    let pair = bucket(slots, key);
    let hit = pair.iter().position(|slot| slot.get().holds(key))?;
    Some(touch(pair, hit))
}

/// Record `verdict` for `key` in a fixed run. Negative verdicts are recorded exactly as positive
/// ones; a full bucket evicts the slot not touched last.
fn record(slots: &[Cell<VerdictSlot>], tally: &Tally, key: VerdictKey, verdict: bool) {
    let pair = bucket(slots, key);
    let held = pair.iter().position(|slot| slot.get().holds(key));
    let free = || pair.iter().position(|slot| !slot.get().occupied);
    let slot = held.or_else(free).unwrap_or_else(|| {
        tally.evicted();
        usize::from(pair[0].get().recent)
    });
    tally.inserted();
    pair[slot].set(VerdictSlot {
        key,
        verdict,
        occupied: true,
        recent: false,
    });
    touch(pair, slot);
}

/// One verdict a branch keeps: its key beside its answer. Laid in the branch's half.
#[derive(Clone, Copy)]
pub(super) struct VerdictEntry {
    key: VerdictKey,
    verdict: bool,
}

impl VerdictEntry {
    pub(super) fn new(key: VerdictKey, verdict: bool) -> Self {
        VerdictEntry { key, verdict }
    }

    pub(super) fn verdict(&self) -> bool {
        self.verdict
    }
}

impl Keyed for VerdictEntry {
    type Key = VerdictKey;

    fn key(&self) -> VerdictKey {
        self.key
    }

    fn word(key: VerdictKey) -> u64 {
        key.fold()
    }
}

/// Mark slot `hit` of a bucket the one touched last, and return its verdict.
fn touch(pair: &[Cell<VerdictSlot>], hit: usize) -> bool {
    let (mut this, mut other) = (pair[hit].get(), pair[1 - hit].get());
    this.recent = true;
    other.recent = false;
    pair[hit].set(this);
    pair[1 - hit].set(other);
    this.verdict
}

/// `test`-only counts of a fixed table's inserts and evictions; nothing outside the test build.
#[derive(Default, Clone)]
struct Tally {
    #[cfg(test)]
    inserts: Cell<u64>,
    #[cfg(test)]
    evictions: Cell<u64>,
}

impl Tally {
    fn inserted(&self) {
        #[cfg(test)]
        self.inserts.set(self.inserts.get() + 1);
    }

    fn evicted(&self) {
        #[cfg(test)]
        self.evictions.set(self.evictions.get() + 1);
    }

    #[cfg(test)]
    fn read(&self) -> (u64, u64) {
        (self.inserts.get(), self.evictions.get())
    }
}

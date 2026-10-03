//! The verdict table: a fixed cache of relation answers the registry owns beside its nodes.
//!
//! Verdicts are keyed by `(subject digest, candidate digest, relation)`: a fixed run of two-slot
//! buckets laid in the region the first time a verdict is recorded, and never resized, so it
//! strands nothing in a bump that releases nothing before the operator run ends. A full bucket
//! evicts the slot not touched last. A verdict over a digest pair is a pure function — once
//! computed it never changes — so verdicts are never load-bearing: a forgotten one costs a re-walk
//! of the relation, never a wrong answer.
//!
//! [`TypeRegistry`](super::registry::TypeRegistry) holds one table and delegates its verdict doors
//! here; the relations that read and record verdicts live in [`order`](super::order),
//! [`ranking`](super::ranking) and [`unify`](super::unify).

use std::cell::Cell;

use crate::memory::BumpAllocator;

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
    ClassAtLeast(u8),
}

impl Relation {
    /// The bits a bucket fold mixes in: distinct per relation and per class.
    fn bits(self) -> u64 {
        match self {
            Relation::Subtype => 0,
            Relation::Fits => 1,
            Relation::MemberAtLeast => 2,
            Relation::ClassAtLeast(class) => 3 + u64::from(class),
        }
    }
}

/// Slots in a registry's verdict table. A power of two, two slots to a bucket.
pub(super) const VERDICT_SLOTS: usize = 1024;

/// One slot of the verdict table: a whole key, its verdict, and the bucket's recency bit.
#[derive(Clone, Copy)]
struct VerdictSlot {
    subject: TypeDigest,
    candidate: TypeDigest,
    relation: Relation,
    verdict: bool,
    occupied: bool,
    /// Set on the slot of its bucket touched last; the other slot is the one an insert evicts.
    recent: bool,
}

impl VerdictSlot {
    const EMPTY: Self = Self {
        subject: TypeDigest(0),
        candidate: TypeDigest(0),
        relation: Relation::Subtype,
        verdict: false,
        occupied: false,
        recent: false,
    };

    fn holds(self, subject: TypeDigest, candidate: TypeDigest, relation: Relation) -> bool {
        self.occupied
            && self.subject == subject
            && self.candidate == candidate
            && self.relation == relation
    }
}

/// The table itself: empty until the first verdict is recorded, then `len` slots long for good.
/// Each slot is copied in and out whole.
pub(super) struct VerdictTable<'run> {
    slots: Cell<&'run [Cell<VerdictSlot>]>,
    len: usize,
    #[cfg(test)]
    evictions: Cell<u64>,
    #[cfg(test)]
    inserts: Cell<u64>,
}

impl<'run> VerdictTable<'run> {
    /// A table that will hold `len` slots once laid.
    pub(super) fn with_slots(len: usize) -> Self {
        assert!(len.is_power_of_two() && len >= 2);
        Self {
            slots: Cell::new(&[]),
            len,
            #[cfg(test)]
            evictions: Cell::new(0),
            #[cfg(test)]
            inserts: Cell::new(0),
        }
    }

    /// The first slot of the bucket a key lands in. A digest is already a uniformly distributed
    /// hash, so the two digests' low words fold together — rotated apart, so `(a, b)` and `(b, a)`
    /// land in different buckets — with the relation mixed in. A slot compares the whole key, so a
    /// fold collision costs a slot and never a wrong verdict.
    fn bucket(&self, subject: TypeDigest, candidate: TypeDigest, relation: Relation) -> usize {
        let fold = (subject.0 as u64).rotate_left(32) ^ (candidate.0 as u64) ^ relation.bits();
        2 * ((fold as usize) & (self.len / 2 - 1))
    }

    /// The recorded verdict for the key, if any. A hit marks its slot the one touched last.
    pub(super) fn get(
        &self,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
    ) -> Option<bool> {
        let table = self.slots.get();
        if table.is_empty() {
            return None;
        }
        let first = self.bucket(subject, candidate, relation);
        let pair = &table[first..first + 2];
        let hit = pair
            .iter()
            .position(|slot| slot.get().holds(subject, candidate, relation))?;
        Some(touch(pair, hit))
    }

    /// Record `verdict` for the key. Negative verdicts are recorded exactly as positive ones. The
    /// table is laid in `bump` on the first record; after that a full bucket evicts the slot not
    /// touched last.
    pub(super) fn record(
        &self,
        bump: BumpAllocator<'run>,
        subject: TypeDigest,
        candidate: TypeDigest,
        relation: Relation,
        verdict: bool,
    ) {
        let mut table = self.slots.get();
        if table.is_empty() {
            table = bump.alloc_slice_fill_with(self.len, |_| Cell::new(VerdictSlot::EMPTY));
            self.slots.set(table);
        }
        let first = self.bucket(subject, candidate, relation);
        let pair = &table[first..first + 2];
        let held = pair
            .iter()
            .position(|slot| slot.get().holds(subject, candidate, relation));
        let free = || pair.iter().position(|slot| !slot.get().occupied);
        let slot = held.or_else(free).unwrap_or_else(|| {
            #[cfg(test)]
            self.evictions.set(self.evictions.get() + 1);
            usize::from(pair[0].get().recent)
        });
        #[cfg(test)]
        self.inserts.set(self.inserts.get() + 1);
        pair[slot].set(VerdictSlot {
            subject,
            candidate,
            relation,
            verdict,
            occupied: true,
            recent: false,
        });
        touch(pair, slot);
    }

    /// `test`-only: how many verdicts were recorded, and how many of those evicted another.
    #[cfg(test)]
    pub(super) fn tally(&self) -> (u64, u64) {
        (self.inserts.get(), self.evictions.get())
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

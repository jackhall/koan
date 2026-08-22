//! The scope's **claim store**: the in-flight binders of the one block that binds into this scope,
//! and nothing else. A claim is not a table entry — the binding maps beside this hold committed
//! bindings only, so each of them states its own exclusivity rule with no in-flight arm to admit.
//!
//! Three parts, each answering exactly one question:
//!
//! - [`ClaimStore::name_claim`] reads `by_name` — the name channel's read path, one hash probe on
//!   the miss that would otherwise raise `UnboundName`. One map covers value and type claims alike,
//!   keyed by the raw [`Symbol`]: the two bindable classes — value tokens and Type tokens —
//!   classify disjoint text, so the two channels cannot collide on a key.
//! - [`ClaimStore::bucket_claim`] reads `by_bucket` — the bucket channel's read path, keyed on the
//!   same full stored run `functions` is, so one key reaches both. A key admits several sibling
//!   binders, each at its own [`BindingIndex`], so the value is a run and the read returns the
//!   earliest-index visible claim: the most-likely-first-finalizer.
//! - `by_statement` is the **retirement** path: a run sized at the block fan-out and indexed by
//!   `BindingIndex`, each entry naming the at-most-three keys its statement claimed plus a live mask
//!   over them. It is the only part keyed by something other than what a reader looks up, which is
//!   what lets a retiring slot find its own claims from the one address it knows about itself.
//!
//! Retirement is therefore an array index and a zero test on the success path — the commit already
//! removed each claim as it wrote — and at most three direct removals otherwise. Nothing is
//! searched in either direction: not the binding tables by producer, and not the store by name.
//!
//! **Drop-freeness.** The store lives inside [`Tables`](super::Tables), under the one
//! `ManuallyDrop` that makes a scope's binding state contribute zero drop glue, so the maps and the
//! statement run carry their vacuous bump-freeing destructors exactly as the binding maps do. The
//! one place that is not enough is `by_bucket`'s **value**: [`bump_table`] proves entry types carry
//! no glue, so the claim run a bucket key maps to wears its own `ManuallyDrop` for
//! [`Bucket`](super::Bucket)'s reason, with the element proof stated below against the element type
//! directly.

use std::mem::ManuallyDrop;

use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::model::{IdentityBuildHasher, Symbol};
use crate::machine::model::{KeyElement, UntypedKey};
use crate::witnessed::{BumpBackedMap, BumpVec};

use super::{BindingIndex, Bindings, bump_table};

/// One in-flight binder's claim: the [`ProducerId`] naming its submission's own installed edge,
/// tagged with the binder's lexical [`BindingIndex`] so the same visibility predicate gates a claim
/// and the binding it becomes. A consumer parks by wiring its **own** edge off this one, inheriting
/// the destination — which is what makes a placeholder park deliver into the scope the name was
/// claimed in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub producer: ProducerId,
    pub index: BindingIndex,
}

const _: () = assert!(!std::mem::needs_drop::<Claim>());

/// `live` bit 0 — the statement's name-channel claim.
const NAME_BIT: u8 = 1 << 0;
/// `live` bits 1 and 2 — the statement's bucket-channel claims, in declaration order. Two is the
/// maximum any binder form reaches (a `UNARY OP` declares the keyword-first list key plus the binary
/// bridge key), which is what keeps the record fixed-size.
const BUCKET_BITS: [u8; 2] = [1 << 1, 1 << 2];

/// One statement's claims: at most one name and at most two bucket keys, all at one
/// [`BindingIndex`]. The keys are the ones the read maps are keyed on, so retirement removes them
/// directly rather than reconstructing them. `live` says which channels are still unretired — a
/// commit clears its own bit as it writes, so a zero mask is the whole of the success path.
#[derive(Clone, Copy)]
struct ClaimRecord<'a> {
    name: Option<Symbol>,
    buckets: [Option<&'a [KeyElement]>; 2],
    live: u8,
}

const _: () = assert!(!std::mem::needs_drop::<ClaimRecord<'static>>());

impl<'a> ClaimRecord<'a> {
    /// A statement that has claimed nothing — what the fan-out sizes the run with, and what a
    /// record is reset to before its statement's first claim lands.
    const EMPTY: ClaimRecord<'a> = ClaimRecord {
        name: None,
        buckets: [None, None],
        live: 0,
    };
}

/// The claims sharing one bucket key, in install order. `ManuallyDrop` for [`Bucket`]'s reason: the
/// run's elements carry no glue (the assert on [`Claim`] above) and its buffer is bump memory the
/// region releases whole, so the suppressed destructor had nothing to do — and suppressing it is
/// what lets a run stand as a [`bump_table`] value at all.
type BucketClaims<'a> = ManuallyDrop<BumpVec<'a, Claim>>;

pub(crate) struct ClaimStore<'a> {
    by_name: BumpBackedMap<'a, Symbol, Claim, IdentityBuildHasher>,
    by_bucket: BumpBackedMap<'a, &'a [KeyElement], BucketClaims<'a>>,
    /// Indexed by [`BindingIndex::idx`]. Sized once at the block fan-out; a statement-at-a-time
    /// door builds no store, so a claim arriving through one grows the run to reach its own index.
    by_statement: BumpVec<'a, ClaimRecord<'a>>,
    /// Whether a block has already fanned out into this scope. The fixed-run indexing rests on a
    /// scope being fanned out into exactly once; this is that fact, asserted rather than assumed.
    fanned_out: bool,
}

impl<'a> ClaimStore<'a> {
    /// An empty store over `brand`'s region — the same bump every binding map's storage lives in.
    pub(super) fn new(brand: RegionBrand<'a>) -> Self {
        ClaimStore {
            by_name: bump_table(brand),
            by_bucket: bump_table(brand),
            by_statement: BumpVec::new_in(brand.allocator()),
            fanned_out: false,
        }
    }

    /// Size the statement run for a block of `statements` statements fanning out into this scope.
    /// Statement `i` submits at chain index `i + 1`, so the run reaches `statements` inclusive.
    pub(super) fn begin_block(&mut self, statements: usize) {
        debug_assert!(
            !std::mem::replace(&mut self.fanned_out, true),
            "a scope is fanned out into exactly once: the statement run is sized at that one \
             fan-out and indexed by BindingIndex",
        );
        self.reserve_through(statements);
    }

    fn reserve_through(&mut self, idx: usize) {
        if self.by_statement.len() <= idx {
            self.by_statement.resize(idx + 1, ClaimRecord::EMPTY);
        }
    }

    /// The record `index`'s claims go in, reset if the statement holds none — so a persistent
    /// scope's later run reuses the slot rather than reading the previous run's spent keys.
    fn record_mut(&mut self, index: BindingIndex) -> &mut ClaimRecord<'a> {
        self.reserve_through(index.idx);
        let record = &mut self.by_statement[index.idx];
        if record.live == 0 {
            *record = ClaimRecord::EMPTY;
        }
        record
    }

    /// The claim standing on `name`, whatever channel it resolves in. Visibility is the caller's:
    /// the resolution walk filters, and the finalize gate's dependency tracking does not.
    pub(super) fn name_claim(&self, name: Symbol) -> Option<Claim> {
        self.by_name.get(&name).copied()
    }

    /// The earliest-index visible claim on a bucket key — the sibling most likely to finalize
    /// first, and so the one a consumer parks on. One hash probe; the run it lands in holds one
    /// entry per sibling binder declaring the key.
    pub(super) fn bucket_claim(
        &self,
        key: &[KeyElement],
        cutoff: Option<usize>,
    ) -> Option<ProducerId> {
        self.by_bucket
            .get(key)?
            .iter()
            .filter(|claim| Bindings::visible(claim.index, cutoff))
            .min_by_key(|claim| claim.index.idx)
            .map(|claim| claim.producer)
    }

    /// Stamp `claim` on `name`. The key is a `Copy` digest, so nothing re-homes. Returns the
    /// standing claim if one already holds the name — the caller rules on whether that is a
    /// re-entry of the same producer or a collision.
    pub(super) fn claim_name(&mut self, name: Symbol, claim: Claim) -> Result<(), Claim> {
        if let Some(standing) = self.by_name.get(&name).copied() {
            return Err(standing);
        }
        self.by_name.insert(name, claim);
        let record = self.record_mut(claim.index);
        record.name = Some(name);
        record.live |= NAME_BIT;
        Ok(())
    }

    /// Stamp `claim` on a bucket key. **Append, never deduplicate**: sibling binders sharing one
    /// inner-call bucket key each claim at their own [`BindingIndex`], and each stands as a wake
    /// source until its own binder retires it.
    pub(super) fn claim_bucket(
        &mut self,
        brand: RegionBrand<'a>,
        bucket: &UntypedKey,
        claim: Claim,
    ) {
        // Probe-then-insert rather than an `entry` call: the key a miss inserts has to be re-homed
        // through the brand, which the entry API has no way to defer. The second hash is paid only
        // on the first claim of a shape.
        if !self.by_bucket.contains_key(bucket.as_slice()) {
            let key: &'a [KeyElement] = brand.allocator().slice(bucket);
            self.by_bucket
                .insert(key, ManuallyDrop::new(BumpVec::new_in(brand.allocator())));
        }
        let (key, claims) = self
            .by_bucket
            .get_key_value_mut(bucket.as_slice())
            .expect("the claim run was just seeded if it was missing");
        claims.push(claim);
        let stored = *key;
        let record = self.record_mut(claim.index);
        let slot = record
            .buckets
            .iter()
            .position(Option::is_none)
            .expect("a binder form declares at most two bucket keys");
        record.buckets[slot] = Some(stored);
        record.live |= BUCKET_BITS[slot];
    }

    /// Retire the name claim `index` stamped, if it is still standing — the name channel's half of
    /// "a commit retires its own claim". One hash removal and one bit, with nothing searched.
    pub(super) fn retire_name(&mut self, name: Symbol, index: BindingIndex) {
        let standing = match self.by_statement.get_mut(index.idx) {
            Some(record)
                if record.live & NAME_BIT != 0 && record.name.is_some_and(|held| held == name) =>
            {
                record.live &= !NAME_BIT;
                true
            }
            _ => false,
        };
        if standing {
            self.by_name.remove(&name);
        }
    }

    /// Retire the bucket claim `index` stamped on `key`, if it is still standing — the bucket
    /// channel's half of the same rule. Sibling claims on the key stand.
    pub(super) fn retire_bucket(&mut self, key: &[KeyElement], index: BindingIndex) {
        let Some(record) = self.by_statement.get_mut(index.idx) else {
            return;
        };
        let held = record
            .buckets
            .iter()
            .position(|bucket| bucket.is_some_and(|stored| key == stored));
        let Some(slot) = held.filter(|slot| record.live & BUCKET_BITS[*slot] != 0) else {
            return;
        };
        record.live &= !BUCKET_BITS[slot];
        let stored = record.buckets[slot].expect("the slot was matched by its stored key");
        self.drop_bucket_claim(stored, index);
    }

    /// Retire every claim `index` still holds. A zero mask is the whole of the success path — the
    /// commit removed each claim as it wrote — and a non-zero one names the at-most-three keys
    /// still standing, each removed from its read map directly.
    pub(super) fn retire_statement(&mut self, index: BindingIndex) {
        let Some(record) = self.by_statement.get(index.idx).copied() else {
            return;
        };
        if record.live == 0 {
            return;
        }
        if record.live & NAME_BIT != 0
            && let Some(name) = record.name
            && self.by_name.get(&name).is_some_and(|c| c.index == index)
        {
            self.by_name.remove(&name);
        }
        for (slot, bit) in BUCKET_BITS.iter().enumerate() {
            if record.live & bit != 0
                && let Some(stored) = record.buckets[slot]
            {
                self.drop_bucket_claim(stored, index);
            }
        }
        self.by_statement[index.idx].live = 0;
    }

    /// Drop `index`'s entry from `stored`'s claim run. The emptied run stays keyed: a reader takes
    /// it for a miss, and a later sibling claim of the same shape reuses it rather than re-homing
    /// the key.
    fn drop_bucket_claim(&mut self, stored: &'a [KeyElement], index: BindingIndex) {
        if let Some(claims) = self.by_bucket.get_mut(stored) {
            claims.retain(|claim| claim.index != index);
        }
    }

    /// Every standing name claim, as `(name, producer)` — the hygiene probe behind
    /// [`Bindings::pending_names`].
    #[cfg(test)]
    pub(super) fn name_claims(&self) -> Vec<(Symbol, Claim)> {
        self.by_name
            .iter()
            .map(|(name, claim)| (*name, *claim))
            .collect()
    }

    /// Every standing claim on one bucket key, in install order.
    #[cfg(test)]
    pub(super) fn bucket_claims(&self, bucket: &UntypedKey) -> Vec<Claim> {
        self.by_bucket
            .get(bucket.as_slice())
            .map(|claims| claims.to_vec())
            .unwrap_or_default()
    }
}

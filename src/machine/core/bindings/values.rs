//! The scope's **value channel**: the storage a value name resolves through, in either of its two
//! representations, and the owner of the value-name claims that go with it.
//!
//! One cell per name answers both of that name's questions. A [`SlotState`] is `Empty`, `Claimed`
//! on the in-flight binder's [`ProducerId`], or `Bound` — so a lookup is one read that says bound,
//! parked, or miss, and a commit retires the claim it satisfies by *replacing* it rather than by
//! removing an entry from a second structure. That is why the claim store beside this
//! ([`ClaimStore`](super::ClaimStore)) keys only the channels whose claims have no such cell: the
//! dispatch buckets and the type names.
//!
//! Two representations, one surface:
//!
//! - [`Keyed`](ValueStore::Keyed) — a [`BumpBackedMap`] on [`ValueSymbol`], for a scope whose
//!   binding set is not lexically fixed: the run root, module and `SIG` bodies, `USING` overlays, a
//!   `CLOSE OVER` block's captured environment.
//! - [`Slotted`](ValueStore::Slotted) — a [`SlotArray`] sized by the body's own
//!   [`SlotLayout`], for a per-call frame whose binders the body enumerates statically. One bump
//!   allocation per activation, a binary search where the keyed form hashes, and **no per-entry
//!   position**: the layout carries each binder's lexical index, so nothing per call restates it.
//!
//! Every verb below is written against the cell, so the two representations differ in how a name is
//! addressed and in nothing else — the visibility rule, the bind-once rule and the claim rules are
//! one implementation each.

use crate::machine::ProducerId;
use crate::machine::model::SlotLayout;
use crate::machine::model::{IdentityBuildHasher, ValueSymbol};
use crate::memory::{BumpBackedMap, RegionBrand, SlotArray, SlotConflict, SlotState, bump_table};

use super::{BindingIndex, Claim, NameLookup, SealedValue};

/// A value binding entry in the **keyed** store: its lexical [`BindingIndex`] and the dormant
/// [`SealedValue`] carrier fusing the bound value with the exact reach description minted for it.
/// The slotted store keeps no such pair — its position comes from the layout — so this is the
/// keyed representation's own cell payload.
///
/// The entry owns **nothing**: liveness for every region the value reaches lives in the binding
/// scope's region-owned union bundle, folded in by the mint that derived the entry's description
/// ([`Scope::mint_retained`](crate::machine::core::Scope::mint_retained)) and dropped whole at
/// region death. Bindings are bind-once and an entry never dies before its scope, so region death
/// and entry death are the same schedule — the entry is `Copy`-cheap to read out and carries no
/// `Drop`. Fusing value and reach in the seal keeps the write door from ever pairing a value with a
/// reach derived for a different value.
pub(super) struct DataEntry<'a> {
    pub(super) index: BindingIndex,
    pub(super) sealed: SealedValue<'a>,
}

/// The keyed store's cell: the same three states the slot array holds, over the entry shape a
/// position-carrying representation needs.
type KeyedCell<'a> = SlotState<DataEntry<'a>, Claim>;

const _: () = assert!(!std::mem::needs_drop::<KeyedCell<'static>>());

/// Where a bound entry sits in the store that holds it, in the form a **copy** of that store
/// addresses it by. A slotted scope copies to a slotted one over the same slot order — the layout
/// travels with it — so its entries copy by slot, in order, with no name resolved at the
/// destination; a keyed one copies by name, at the position stored beside the entry.
#[derive(Clone, Copy)]
pub(crate) enum ValueAddress {
    /// A name-addressed cell, at the lexical position stored beside it.
    Keyed(BindingIndex),
    /// Slot `slot` of the store's layout, at the position that layout pairs with it.
    Slot { slot: usize, index: BindingIndex },
}

impl ValueAddress {
    /// The entry's lexical position — what the `idx < cutoff` visibility rule reads, whichever
    /// representation holds it.
    pub(crate) fn index(self) -> BindingIndex {
        match self {
            ValueAddress::Keyed(index) | ValueAddress::Slot { index, .. } => index,
        }
    }
}

/// A write refused because the name is already spoken for — a committed binding, or a claim naming
/// a different binder. The caller renders the `Rebind` diagnostic, since only it holds the
/// registries the name is spelled through.
pub(super) struct Rebind;

pub(super) enum ValueStore<'a> {
    /// Name-addressed. `claimed` counts the cells currently in the `Claimed` state, so the
    /// "no binder is still in flight" read is O(1) here exactly as it is for the array.
    Keyed {
        cells: BumpBackedMap<'a, ValueSymbol, KeyedCell<'a>, IdentityBuildHasher>,
        claimed: usize,
    },
    /// Layout-addressed. The layout is the body's, homed in the body node's or the callable's own
    /// region — never in this scope's, which a spliced-out array could outlive.
    Slotted {
        layout: &'a SlotLayout<'a>,
        cells: SlotArray<'a, SealedValue<'a>, ProducerId>,
    },
}

impl<'a> ValueStore<'a> {
    /// An empty name-addressed store over `brand`'s region.
    pub(super) fn keyed(brand: RegionBrand<'a>) -> Self {
        ValueStore::Keyed {
            cells: bump_table(brand),
            claimed: 0,
        }
    }

    /// A layout-sized slot array over `brand`'s region — one allocation, and none at all for a body
    /// that binds no value.
    pub(super) fn slotted(brand: RegionBrand<'a>, layout: &'a SlotLayout<'a>) -> Self {
        ValueStore::Slotted {
            layout,
            cells: SlotArray::new(brand.allocator(), layout.len()),
        }
    }

    /// This store's layout, for the representation that has one — what a copied scope re-homes.
    pub(super) fn layout(&self) -> Option<&'a SlotLayout<'a>> {
        match self {
            ValueStore::Keyed { .. } => None,
            ValueStore::Slotted { layout, .. } => Some(layout),
        }
    }

    /// Per-scope lookup: one cell read answering `Bound`, `Parked`, or a miss the caller keeps
    /// walking past. `cutoff = None` means the reader is off this scope's chain, so everything is
    /// visible; `Some(c)` admits a binder at position `idx < c`.
    pub(super) fn lookup(
        &self,
        name: ValueSymbol,
        cutoff: Option<usize>,
    ) -> Option<NameLookup<SealedValue<'a>>> {
        match self {
            ValueStore::Keyed { cells, .. } => match cells.get(&name)? {
                SlotState::Empty => None,
                SlotState::Claimed(claim) => {
                    visible(claim.index.idx, cutoff).then_some(NameLookup::Parked(claim.producer))
                }
                SlotState::Bound(entry) => visible(entry.index.idx, cutoff)
                    .then(|| NameLookup::Bound(entry.sealed.duplicate())),
            },
            ValueStore::Slotted { layout, cells } => {
                let slot = layout.slot_of(name)?;
                if !visible(layout.position(slot), cutoff) {
                    return None;
                }
                match cells.get(slot) {
                    SlotState::Empty => None,
                    SlotState::Claimed(producer) => Some(NameLookup::Parked(*producer)),
                    SlotState::Bound(sealed) => Some(NameLookup::Bound(sealed.duplicate())),
                }
            }
        }
    }

    /// The **bound** carrier for `name`, ignoring claims — the module-member read, where the map
    /// holds a finalized module and a claim cannot surface.
    pub(super) fn bound(
        &self,
        name: ValueSymbol,
        cutoff: Option<usize>,
    ) -> Option<SealedValue<'a>> {
        match self.lookup(name, cutoff)? {
            NameLookup::Bound(sealed) => Some(sealed),
            NameLookup::Parked(_) => None,
        }
    }

    /// Whether a **committed** binding stands on `name`, at any position.
    #[cfg(test)]
    pub(super) fn is_bound(&self, name: ValueSymbol) -> bool {
        match self {
            ValueStore::Keyed { cells, .. } => {
                cells.get(&name).is_some_and(|cell| cell.bound().is_some())
            }
            ValueStore::Slotted { layout, cells } => layout
                .slot_of(name)
                .is_some_and(|slot| cells.get(slot).bound().is_some()),
        }
    }

    /// Stamp the in-flight binder `producer`'s claim on `name` at `index`. A standing claim naming
    /// the *same* binder is that stamp arriving twice, not a second declaration, so it succeeds;
    /// anything else — a committed binding, a different binder's claim — is a `Rebind`.
    pub(super) fn claim(
        &mut self,
        name: ValueSymbol,
        producer: ProducerId,
        index: BindingIndex,
    ) -> Result<(), Rebind> {
        let outcome = match self {
            ValueStore::Keyed { cells, claimed } => {
                let cell = cells.entry(name).or_insert(SlotState::Empty);
                let outcome = cell.claim(Claim { producer, index });
                *claimed += usize::from(matches!(outcome, Ok(true)));
                outcome.map(|_| ()).map_err(|conflict| match conflict {
                    SlotConflict::Claimed(standing) => Some(standing.producer),
                    SlotConflict::Bound => None,
                })
            }
            ValueStore::Slotted { layout, cells } => {
                let slot = layout
                    .slot_of(name)
                    .expect("a slotted frame's binders and its layout share one statement reader");
                debug_assert!(
                    layout.position(slot) <= index.idx,
                    "a slot's position is its *first* binder's, so a later binder of the same \
                     name submits at or after it",
                );
                cells
                    .claim(slot, producer)
                    .map_err(|conflict| match conflict {
                        SlotConflict::Claimed(standing) => Some(standing),
                        SlotConflict::Bound => None,
                    })
            }
        };
        match outcome {
            Ok(()) => Ok(()),
            Err(Some(standing)) if standing == producer => Ok(()),
            Err(_) => Err(Rebind),
        }
    }

    /// Commit `name` → `sealed` as a bind-once binding, **retiring the binder's own claim** by
    /// replacing it. A committed name rebinds.
    pub(super) fn bind(
        &mut self,
        name: ValueSymbol,
        index: BindingIndex,
        sealed: SealedValue<'a>,
    ) -> Result<(), Rebind> {
        match self {
            ValueStore::Keyed { cells, claimed } => {
                let cell = cells.entry(name).or_insert(SlotState::Empty);
                let retired = cell.bind(DataEntry { index, sealed }).map_err(|_| Rebind)?;
                *claimed -= usize::from(retired);
                Ok(())
            }
            ValueStore::Slotted { layout, cells } => {
                let slot = layout
                    .slot_of(name)
                    .expect("a slotted frame's binders and its layout share one statement reader");
                debug_assert!(
                    layout.position(slot) <= index.idx,
                    "a slot's position is its *first* binder's, so a later writer of the same \
                     name submits at or after it",
                );
                cells.bind(slot, sealed).map_err(|_| Rebind)
            }
        }
    }

    /// Drop the claim `index` stamped on `name`, if it is still standing — what a binder that
    /// terminalizes without committing leaves behind. A committed cell is untouched.
    pub(super) fn retire_claim(&mut self, name: ValueSymbol, index: BindingIndex) {
        match self {
            ValueStore::Keyed { cells, claimed } => {
                let Some(cell) = cells.get_mut(&name) else {
                    return;
                };
                // Only the stamping statement's own claim retires here: a later binder of the name
                // that collided left the first claim standing, and it is not this one's to drop.
                if !matches!(cell, SlotState::Claimed(standing) if standing.index == index) {
                    return;
                }
                *claimed -= usize::from(cell.retire_claim());
            }
            ValueStore::Slotted { layout, cells } => {
                let Some(slot) = layout.slot_of(name) else {
                    return;
                };
                debug_assert!(layout.position(slot) <= index.idx);
                // No ownership test is needed: a later binder of the name collided on the claim
                // and so left no claim record for its statement, and the store is reached only
                // through the record its own claim wrote.
                cells.retire_claim(slot);
            }
        }
    }

    /// Whether no binder is still in flight into this channel — one counter read either way.
    pub(super) fn has_no_claims(&self) -> bool {
        match self {
            ValueStore::Keyed { claimed, .. } => *claimed == 0,
            ValueStore::Slotted { cells, .. } => cells.claimed_count() == 0,
        }
    }

    /// Every committed binding, as `(name, address, carrier)` — the slotted arm **in slot order**,
    /// which is what lets a copy fill its own array positionally. Visibility is the caller's.
    pub(super) fn for_each_bound(
        &self,
        mut f: impl FnMut(ValueSymbol, ValueAddress, &SealedValue<'a>),
    ) {
        match self {
            ValueStore::Keyed { cells, .. } => {
                for (name, cell) in cells.iter() {
                    if let SlotState::Bound(entry) = cell {
                        f(*name, ValueAddress::Keyed(entry.index), &entry.sealed);
                    }
                }
            }
            ValueStore::Slotted { layout, cells } => {
                for (slot, cell) in cells.iter() {
                    if let SlotState::Bound(sealed) = cell {
                        let index = BindingIndex::value(layout.position(slot));
                        f(
                            layout.name(slot),
                            ValueAddress::Slot { slot, index },
                            sealed,
                        );
                    }
                }
            }
        }
    }

    /// The environment copy's write: place a rebuilt binding at the address its source sat at. A
    /// slotted destination writes the slot directly — the layout it re-homed from the source pairs
    /// that slot with the same name and the same position, so nothing is resolved here.
    pub(super) fn insert_copied(
        &mut self,
        name: ValueSymbol,
        at: ValueAddress,
        sealed: SealedValue<'a>,
    ) {
        match (self, at) {
            (ValueStore::Keyed { cells, .. }, ValueAddress::Keyed(index)) => {
                debug_assert!(
                    !cells.contains_key(&name),
                    "an environment copy fills an empty table, one entry per source name",
                );
                cells.insert(name, SlotState::Bound(DataEntry { index, sealed }));
            }
            (ValueStore::Slotted { layout, cells }, ValueAddress::Slot { slot, index }) => {
                debug_assert_eq!(layout.name(slot), name);
                // Equality, not the bound the binder paths take: the source address was minted as
                // its own layout's position, and the re-homed layout is that layout.
                debug_assert_eq!(layout.position(slot), index.idx);
                cells
                    .bind(slot, sealed)
                    .unwrap_or_else(|_| panic!("an environment copy fills an empty slot array"));
            }
            _ => unreachable!("a copied scope takes its source's own representation"),
        }
    }

    /// The producer behind every claim visible at `cutoff` — the value channel's contribution to
    /// the implicit-close wait set.
    pub(super) fn for_each_visible_claim(
        &self,
        cutoff: Option<usize>,
        mut f: impl FnMut(ProducerId),
    ) {
        match self {
            ValueStore::Keyed { cells, .. } => {
                for cell in cells.values() {
                    if let SlotState::Claimed(claim) = cell
                        && visible(claim.index.idx, cutoff)
                    {
                        f(claim.producer);
                    }
                }
            }
            ValueStore::Slotted { layout, cells } => {
                for (slot, cell) in cells.iter() {
                    if let SlotState::Claimed(producer) = cell
                        && visible(layout.position(slot), cutoff)
                    {
                        f(*producer);
                    }
                }
            }
        }
    }

    /// An upper bound on the committed names here, in one length read — a keyed store's cell count
    /// (claimed and retired cells included) or a slotted one's slot count. What a capture snapshot
    /// sizes its buffer against before it walks.
    pub(super) fn bound_capacity_hint(&self) -> usize {
        match self {
            ValueStore::Keyed { cells, .. } => cells.len(),
            ValueStore::Slotted { layout, .. } => layout.len(),
        }
    }

    /// How many names are committed here — the count the fixtures read in place of a raw map.
    #[cfg(test)]
    pub(super) fn bound_count(&self) -> usize {
        let mut count = 0;
        self.for_each_bound(|_, _, _| count += 1);
        count
    }

    /// Every standing claim, as `(name, claim)` — the hygiene probe's value half.
    #[cfg(test)]
    pub(super) fn claims(&self) -> Vec<(ValueSymbol, Claim)> {
        let mut standing = Vec::new();
        match self {
            ValueStore::Keyed { cells, .. } => {
                for (name, cell) in cells.iter() {
                    if let SlotState::Claimed(claim) = cell {
                        standing.push((*name, *claim));
                    }
                }
            }
            ValueStore::Slotted { layout, cells } => {
                for (slot, cell) in cells.iter() {
                    if let SlotState::Claimed(producer) = cell {
                        standing.push((
                            layout.name(slot),
                            Claim {
                                producer: *producer,
                                index: BindingIndex::value(layout.position(slot)),
                            },
                        ));
                    }
                }
            }
        }
        standing
    }
}

/// Visibility predicate: `cutoff = None` (the reader is off this scope's chain, so the scope is
/// complete) ⇒ visible; `Some(c)` ⇒ `idx < c`.
fn visible(idx: usize, cutoff: Option<usize>) -> bool {
    match cutoff {
        None => true,
        Some(c) => idx < c,
    }
}

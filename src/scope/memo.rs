//! The **decision memo**: what the cells of a tail chain decided in their activations, kept in the
//! region their results are built in so a later cell running the same body over the same bindings
//! reuses it instead of walking ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! A memo holds up to [`CAPACITY`] entries, one per body, the least recently used dropped. An
//! [`Entry`] is keyed by the body's segment, the arena node its cell runs under, and every name
//! the layers beneath the cell's first event bind, by content: what a callable's encloser decided,
//! or a block's frame. It holds one [`Decision`] per binding event, in the order the cell met
//! them. A cell reuses an entry's decisions while each event's site and bound names match; from
//! the first that differs it decides, and the entry it lays keeps the decisions before that one.
//!
//! A memo is immutable once laid, save its clock: a miss lays a new memo beside the old, sharing
//! every entry it keeps. Its ticks are `Cell<u32>`, which name no lifetime, so a memo stays
//! covariant and a cell may hold one pinned at its own brand. Where it rests, and how a cell
//! reaches it, is `program`'s: its region header's [`memo`](super::HalfHeader::memo).

use std::cell::Cell;

use crate::memory::{Bump, BumpVec, DropFree, Lays, collect, covariant, reattachable, resident};
use crate::type_lattice::{DeclaredType, Parametric};
use crate::values::ArenaRef;

use super::facts::{Binding, Bound, Unknown};
use super::records::Records;
use super::relay::Relay;
use super::settlement_facts::Activated;
use super::shape::Segment;

/// How many entries a memo keeps.
pub const CAPACITY: usize = 8;

/// Where a binding event fires: a body's entry, or the statements it ran up to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EventSite {
    segment: Segment,
    at: u32,
}

impl EventSite {
    /// The entry of the body `segment`.
    pub fn entry(segment: Segment) -> Self {
        EventSite {
            segment,
            at: u32::MAX,
        }
    }

    /// The seam after the body `segment` ran its statements up to `ran`.
    pub fn seam(segment: Segment, ran: u32) -> Self {
        EventSite { segment, at: ran }
    }

    /// The body the event fires in.
    pub fn segment(self) -> Segment {
        self.segment
    }
}

/// What an entry is found by: the body, the arena node its cell runs under, and the layers beneath
/// the cell's first event.
#[derive(Clone, Copy)]
pub struct MemoKey<'k> {
    pub segment: Segment,
    pub arena: ArenaRef,
    pub below: Option<&'k Activated<'k>>,
}

/// The family of a region's memo, as a door reads it out of the region's header.
pub struct MemoFamily;
reattachable!(MemoFamily => Option<&'cell DecisionMemo<'cell>>);
// A memo is pinned at a reader's shorter brand.
covariant!(MemoFamily);
impl DropFree for MemoFamily {}

/// The family of the entry a miss laid in a region's memo.
pub struct EntryFamily;
reattachable!(EntryFamily => Option<&'cell Entry<'cell>>);
covariant!(EntryFamily);
impl DropFree for EntryFamily {}

/// One memo: its entries, beside a tick each, and the clock that issued the last tick.
pub struct DecisionMemo<'a> {
    clock: Cell<u32>,
    slots: &'a [MemoSlot<'a>],
}

struct MemoSlot<'a> {
    tick: Cell<u32>,
    entry: &'a Entry<'a>,
}

/// What the cells running one body over one key decided, event by event.
pub struct Entry<'a> {
    segment: Segment,
    arena: ArenaRef,
    /// Every name the layers beneath the first event bound, innermost layer first.
    below: &'a [(Unknown, Bound<'a>)],
    decisions: &'a [&'a Decision<'a>],
}

/// One binding event's decision: where it fired, and the records its walk laid, whose binding is
/// the names it bound.
pub struct Decision<'a> {
    site: EventSite,
    records: Records<'a>,
}

impl<'a> DecisionMemo<'a> {
    /// The entry `key` finds, its tick advanced.
    pub fn find(&self, key: MemoKey<'_>) -> Option<&'a Entry<'a>> {
        let slot = self.slots.iter().find(|slot| slot.entry.keyed(key))?;
        self.clock.set(self.clock.get() + 1);
        slot.tick.set(self.clock.get());
        Some(slot.entry)
    }
}

impl<'a> Entry<'a> {
    /// The decision the entry holds for its cell's `at`-th event.
    pub fn decision(&self, at: u32) -> Option<&'a Decision<'a>> {
        self.decisions.get(at as usize).copied()
    }

    /// Hand `each` every type the entry's decisions name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for decision in self.decisions {
            decision.records.each_type(each);
            each_bound_type(decision.records.binding().entries(), each);
        }
    }

    fn keyed(&self, key: MemoKey<'_>) -> bool {
        self.segment == key.segment && self.arena == key.arena && same_below(self.below, key.below)
    }
}

impl<'a> Decision<'a> {
    /// Whether this decision is the one an event at `site` binding `own` would walk to.
    pub fn matches(&self, site: EventSite, own: &[(Unknown, Bound<'_>)]) -> bool {
        let binding = self.records.binding();
        self.site == site
            && binding.entries().len() == own.len()
            && (own.iter()).all(|(unknown, bound)| {
                binding
                    .bound(*unknown)
                    .is_some_and(|held| held.same_as(bound))
            })
    }

    /// This decision as its cell's layer over `below`.
    pub fn over(&self, below: Option<&'a Activated<'a>>) -> Activated<'a> {
        Activated::over(below, self.records)
    }
}

/// Whether `held`, an entry's names below, are the names the layers `below` bind, in order.
fn same_below(held: &[(Unknown, Bound<'_>)], below: Option<&Activated<'_>>) -> bool {
    let mut held = held.iter();
    let mut at = below;
    while let Some(layer) = at {
        for (unknown, bound) in layer.records().binding().entries() {
            match held.next() {
                Some((mine, theirs)) if mine == unknown && theirs.same_as(bound) => {}
                _ => return false,
            }
        }
        at = layer.below();
    }
    held.next().is_none()
}

/// `old`, a memo at the brand `lay` lays at, with the entry `key` finds extended by `decided`,
/// the cell's `matched`-th event, which fired at `site`: a new memo and the entry it now holds.
/// The entry keeps the decisions before `matched` that `cursor`, the entry the cell has read so
/// far, holds: shared where `old`'s entry for `key` holds the same ones, else laid again. A new
/// entry takes the least recently used slot of a full memo. `adopt` is handed every type what is
/// laid again names, for the caller to make resolve at `lay`'s brand.
#[allow(clippy::too_many_arguments)]
pub fn extended<'to, L: Lays<'to>>(
    lay: L,
    old: Option<&'to DecisionMemo<'to>>,
    key: MemoKey<'_>,
    cursor: Option<&Entry<'_>>,
    matched: u32,
    site: EventSite,
    decided: &Activated<'_>,
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) -> (&'to DecisionMemo<'to>, &'to Entry<'to>) {
    let scratch = Bump::new();
    let mut relay = Relay::new(lay, &scratch, Binding::EMPTY);
    let slots = old.map_or(&[][..], |old| old.slots);
    let kept = slots.iter().position(|slot| slot.entry.keyed(key));
    let prefix = &cursor.map_or(&[][..], |cursor| cursor.decisions)[..matched as usize];
    let shared = kept.map(|at| slots[at].entry.decisions).filter(|held| {
        held.len() >= prefix.len()
            && (held.iter().zip(prefix)).all(|(held, read)| std::ptr::addr_eq(*held, *read))
    });
    let mut decisions = BumpVec::with_capacity_in(prefix.len() + 1, &scratch);
    match shared {
        Some(held) => decisions.extend_from_slice(&held[..prefix.len()]),
        None => decisions.extend(prefix.iter().map(|decision| {
            decision.records.each_type(adopt);
            each_bound_type(decision.records.binding().entries(), adopt);
            relaid(lay, &scratch, &mut relay, decision.site, &decision.records)
        })),
    }
    decided.records().each_type(adopt);
    each_bound_type(decided.records().binding().entries(), adopt);
    decisions.push(relaid(lay, &scratch, &mut relay, site, decided.records()));
    let below = match kept {
        Some(at) => slots[at].entry.below,
        None => {
            let mut below = BumpVec::new_in(&scratch);
            let mut at = key.below;
            while let Some(layer) = at {
                for (unknown, bound) in layer.records().binding().entries() {
                    adopt_bound(bound, adopt);
                    below.push((*unknown, relay.bound(bound)));
                }
                at = layer.below();
            }
            collect(lay, below.iter().copied())
        }
    };
    let entry = resident(
        lay,
        Entry {
            segment: key.segment,
            arena: key.arena,
            below,
            decisions: collect(lay, decisions.iter().copied()),
        },
    );
    let clock = old.map_or(0, |old| old.clock.get()) + 1;
    // The slot the entry lands in: its own key's, else a free one, else the least recently used.
    let into = kept.unwrap_or(match slots.len() < CAPACITY {
        true => slots.len(),
        false => (0..slots.len())
            .min_by_key(|at| slots[*at].tick.get())
            .expect("a full memo has a slot"),
    });
    let len = slots.len().max(into + 1);
    let memo = resident(
        lay,
        DecisionMemo {
            clock: Cell::new(clock),
            slots: lay.fill(len, |at| match at == into {
                true => MemoSlot {
                    tick: Cell::new(clock),
                    entry,
                },
                false => MemoSlot {
                    tick: Cell::new(slots[at].tick.get()),
                    entry: slots[at].entry,
                },
            }),
        },
    );
    (memo, entry)
}

/// `records`, a decision at `site`, laid again through `relay`.
fn relaid<'to, L: Lays<'to>>(
    lay: L,
    scratch: &Bump,
    relay: &mut Relay<'to, '_, '_, L>,
    site: EventSite,
    records: &Records<'_>,
) -> &'to Decision<'to> {
    let records = records.relaid(lay, scratch, relay);
    resident(lay, Decision { site, records })
}

fn each_bound_type(
    bound: &[(Unknown, Bound<'_>)],
    adopt: &mut dyn FnMut(DeclaredType<Parametric>),
) {
    bound
        .iter()
        .for_each(|(_, bound)| adopt_bound(bound, adopt));
}

fn adopt_bound(bound: &Bound<'_>, adopt: &mut dyn FnMut(DeclaredType<Parametric>)) {
    match bound {
        Bound::Type(kt) | Bound::Module { carried: kt, .. } => adopt((*kt).into()),
    }
}

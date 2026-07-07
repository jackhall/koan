//! [`CarrierWitness`] — the value-carrier witness that splits **liveness pins** from **borrow
//! reach**. A single [`FrameSet`] cannot play both roles: a consumer that folds a carrier's reach
//! into its own reach-set must be able to subtract "the frame that only *hosts* the value" from "the
//! frames the value genuinely borrows into". So the carrier lane witnesses with a `{ pins, reach }`
//! pair — `reach` is the exact set of regions the value's interior `&'a` borrows point into (folded
//! downstream), `pins` is liveness-only backing (the residence frame, the owned backing of a severed
//! top node) that keeps the value alive without ever counting as reach.
//!
//! This is the [`Witness`] the carrier families ([`CarriedFamily`](crate::machine::model::values::CarriedFamily)
//! and its build-operand peers) bundle. `reach` keeps its role as the storable, propagatable reach
//! (a binding row's stored reach, a scope's reach-set are plain [`FrameSet`]s folded out of it); the
//! `pins` never leave the carrier — they die with it.

use std::rc::Rc;

use crate::machine::model::types::KType;
use crate::machine::model::values::KObject;
use crate::witnessed::{ComposeWitness, Reattachable, SetWitness, UnionWitness, Witness};

use super::arena::{FrameSet, FrameStorage, KoanRegion};

/// One liveness pin backing a carrier's value: a residence frame the value still lives in, or the
/// owned `'static`-erased backing of a top node copied out of a dying frame (the finalize sever).
/// Every arm is a [`StableDeref`](stable_deref_trait::StableDeref) `Rc`, so holding it keeps its
/// pointee live and fixed-address — the [`Witness`] obligation, discharged per arm.
#[derive(Clone)]
pub enum CarrierPin {
    /// A residence / producer frame whose region the value lives in. Folds like a reach member (it
    /// pins a region) when a consumer walks the pins, so a frame pin never masquerades as reach yet
    /// still keeps the region alive.
    Frame(Rc<FrameStorage>),
    /// The owned backing of an object top node severed from its dying frame — the value is read
    /// through this `Rc`, so holding it is what keeps the severed node alive.
    Object(Rc<KObject<'static>>),
    /// The owned backing of a type top node severed from its dying frame — the type-channel twin of
    /// [`CarrierPin::Object`].
    Type(Rc<KType<'static>>),
}

/// A value carrier's witness: liveness pins plus the exact borrow reach.
///
/// - `reach` names every region the value's interior borrows point into — the propagatable half a
///   downstream mint ([`Scope::host_reach_of`](crate::machine::core::Scope), a binding's stored
///   reach) reads back out. Exact by construction: a region-pure value's reach is empty.
/// - `pins` keeps the value's backing alive (residence frame, severed owned node) without ever being
///   treated as reach. Binds and scope reach-sets never propagate `pins`; a `Frame` pin does keep
///   its region alive when a fold walks it, but it is deposited as a pin, not folded as reach.
pub struct CarrierWitness {
    pins: Vec<CarrierPin>,
    reach: FrameSet,
}

impl CarrierWitness {
    /// A witness pinning nothing and reaching nowhere — the frameless / run-region terminal whose
    /// backing outlives the carrier, and the `Default` element.
    pub fn empty() -> Self {
        CarrierWitness {
            pins: Vec::new(),
            reach: FrameSet::empty(),
        }
    }

    /// A witness pinning one residence frame and reaching nowhere — the run-loop step pin lifting a
    /// dep's producer frame in as a liveness pin. The inherent twin of the [`SetWitness::singleton`]
    /// impl, so callers need not bring the trait into scope.
    pub fn singleton(owner: Rc<FrameStorage>) -> Self {
        CarrierWitness {
            pins: vec![CarrierPin::Frame(owner)],
            reach: FrameSet::empty(),
        }
    }

    /// A witness carrying only a borrow reach, no residence pins — a value born already naming the
    /// regions it borrows into (an FN/MODULE definition resealing its home region into reach).
    pub fn reach_only(reach: FrameSet) -> Self {
        CarrierWitness {
            pins: Vec::new(),
            reach,
        }
    }

    /// A resident value's witness: its home frame as a **residence pin** (liveness-only — the value
    /// lives in that region but the pin is not propagated as reach) plus its exact borrow `reach`. The
    /// pivot from [`Self::reach_only`]: a resident value no longer folds its home into reach, so a
    /// fully-owned value's reach is empty and the finalize gate severs it from a dying home frame. A
    /// value that genuinely borrows into home carries home in `reach` too (the materialized bind bit),
    /// so the gate keeps it.
    pub fn residence(home: Rc<FrameStorage>, reach: FrameSet) -> Self {
        CarrierWitness {
            pins: vec![CarrierPin::Frame(home)],
            reach,
        }
    }

    /// Whether this witness pins and reaches nothing — the frameless / run-region terminal, whose
    /// backing outlives the carrier so no re-home is needed.
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.reach.is_empty()
    }

    /// The exact borrow reach — the propagatable half a downstream fold reads out.
    pub fn reach(&self) -> &FrameSet {
        &self.reach
    }

    /// Whether the **reach** already names `region` — reach members only, ignoring residence pins.
    /// The finalize gate ("does the value genuinely borrow into the frame it resides in?") and the
    /// bind-bit derivation both key on this.
    pub fn reach_covers(&self, region: &KoanRegion) -> bool {
        self.reach.pins_region(region)
    }

    /// Whether **anything** — a reach member or a `Frame` pin's owner chain — keeps `region` alive.
    /// The step-liveness query, where a residence pin counts just as much as a borrow.
    pub fn covers(&self, region: &KoanRegion) -> bool {
        self.reach.pins_region(region)
            || self.pins.iter().any(|pin| match pin {
                CarrierPin::Frame(frame) => frame.pins_region(region),
                CarrierPin::Object(_) | CarrierPin::Type(_) => false,
            })
    }

    /// The pins backing this carrier — the residence frames and severed owned nodes a consumer fold
    /// routes (frame pins through the omission predicate, owned backings to a scope deposit list).
    pub(crate) fn pins(&self) -> &[CarrierPin] {
        &self.pins
    }

    /// The finalize sever's witness rebuild. The value's top node has been copied out of its origin
    /// frame `producer` into an owned `backing`, and the gate that reached here already proved the
    /// value's **reach** does not cover `producer` — so nothing the carrier holds still borrows into
    /// `producer`, and its residence can be released: drop every `Frame` pin that keeps `producer`
    /// alive. Foreign `Frame` pins (borrows into other live regions), owned pins, and the exact reach
    /// are kept verbatim, and `backing` is added — the pin that keeps the copied top node alive.
    pub(crate) fn severed(mut self, producer: &KoanRegion, backing: CarrierPin) -> Self {
        self.pins.retain(|pin| match pin {
            CarrierPin::Frame(frame) => !frame.pins_region(producer),
            CarrierPin::Object(_) | CarrierPin::Type(_) => true,
        });
        self.pins.push(backing);
        self
    }

    /// Collapse to a plain [`FrameSet`] naming every region this witness keeps alive through a frame
    /// (its reach ∪ every `Frame` pin's owner) — the reach a site threads onward as a stored
    /// [`FrameSet`] when it has no scope to home-omit against. Owned (`Object`/`Type`) backings carry
    /// no region and drop out.
    pub(crate) fn to_reach_frameset(&self) -> FrameSet {
        let mut reach = self.reach.clone();
        for pin in &self.pins {
            if let CarrierPin::Frame(frame) = pin {
                reach = FrameSet::union(&reach, &FrameSet::singleton(Rc::clone(frame)));
            }
        }
        reach
    }

    /// The set union of `left` and `right`: concatenate their pins and [`FrameSet::union`] their
    /// reaches. The inherent form the carrier lane and the run-loop step pin call directly; the
    /// [`UnionWitness`] impl delegates here.
    pub fn union(left: &Self, right: &Self) -> Self {
        let mut pins = left.pins.clone();
        pins.extend(right.pins.iter().cloned());
        CarrierWitness {
            pins,
            reach: FrameSet::union(&left.reach, &right.reach),
        }
    }
}

impl Default for CarrierWitness {
    fn default() -> Self {
        Self::empty()
    }
}

impl Clone for CarrierWitness {
    fn clone(&self) -> Self {
        CarrierWitness {
            pins: self.pins.clone(),
            reach: self.reach.clone(),
        }
    }
}

// SAFETY: every pin is a `StableDeref` `Rc` (a frame, or an owned `'static`-erased top node) and
// every reach member is an `Rc<FrameStorage>`, so holding the witness keeps every pinned pointee and
// every reached region's storage live and at a fixed address. The empty witness carries no pin — a
// frameless / run-region terminal is backed by storage that outlives the carrier, so no held pin is
// required.
unsafe impl Witness for CarrierWitness {}

// SAFETY: `singleton(owner)` holds `owner` as a `Frame` pin, so the `Witness` impl above pins
// `owner`'s region for as long as the set is held. This is the semantic pivot: a yoked
// construction's own frame is **residence** (a pin), not **reach** — the value built inside the
// brand borrows nothing foreign, so its reach stays empty.
unsafe impl SetWitness<Rc<FrameStorage>> for CarrierWitness {
    fn singleton(single: Rc<FrameStorage>) -> Self {
        CarrierWitness {
            pins: vec![CarrierPin::Frame(single)],
            reach: FrameSet::empty(),
        }
    }
}

// SAFETY: `union` keeps every pin of both operands (concatenation) and the [`FrameSet`] union of
// both reaches, so holding the result keeps every region either input pinned alive.
unsafe impl UnionWitness for CarrierWitness {
    fn union(left: &Self, right: &Self) -> Self {
        Self::union(left, right)
    }
}

// SAFETY: identical to the `UnionWitness` impl above — plain union already keeps every region either
// input pinned alive, regardless of `dest`. Transitional: `CarrierWitness` collapses to the library
// `Carrier` (whose `ComposeWitness` impl genuinely mints) in the next phase of this item.
unsafe impl<B: Reattachable> ComposeWitness<B> for CarrierWitness {
    fn compose<'b>(left: &Self, right: &Self, _dest: &B::At<'b>) -> Self {
        Self::union(left, right)
    }
}

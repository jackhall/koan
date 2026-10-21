//! What a resumption settles ([the build module's design](../build/README.md#type-facts-and-the-solver)):
//! each body it re-walked, by segment, the facts it re-ran beside the residual it leaves, and the
//! [`Resumption`] a pass runs over the ones it extends.
//!
//! A resumption writes beside the load's write-once cells, never into them, so a fact reads as
//! the newest resumption that re-ran it, else as the load left it. The vocabulary these records
//! name — unknowns, frontiers, residuals, bindings — is [`facts`](super::facts)'s.

use crate::type_lattice::{KType, Parametric};

use super::facts::{Frontier, OldBinding, Residual, Unknown};
use super::keyed::Keyed;
use super::shape::{Segment, ShapeError, Site, Slot};
use super::typed::{
    Resolution, ShapeFacts, StaticCallable, StaticRegistered, StaticSolution, StaticType, Statics,
};

/// What a resumption settled about one body's type channel cells: each cell it re-ran, by where
/// it lies, beside the body's residual as the resumption leaves it.
#[derive(Clone, Copy)]
pub struct ChannelSettled<'a> {
    pub declared: Keyed<'a, (Slot, StaticType<'a>)>,
    pub registered: Keyed<'a, (Slot, StaticRegistered<'a>)>,
    pub expressions: Keyed<'a, (Site, StaticType<'a>)>,
    /// A callable body's type, and its own group as its body reads it, where it re-ran.
    pub callable: Option<StaticCallable<'a>>,
    pub levels: Option<&'a [Parametric]>,
    pub residual: Residual<'a>,
    /// A quote's code's refusal, where its code no longer types.
    pub refusal: Option<&'a ShapeError<'a>>,
}

/// What a resumption settled about one body's value channel: the body's cell as the resumption
/// leaves it, beside its residual.
#[derive(Clone, Copy)]
pub struct StaticsSettled<'a> {
    pub statics: Statics<'a>,
    pub residual: Residual<'a>,
}

/// What one resumption settles: its binding, and each body it re-walked, by segment — the
/// facts it re-ran beside the residual it leaves. A fact it did not re-run reads as the
/// resumption it extends left it, and as the load left it below them all.
#[derive(Clone, Copy)]
pub struct Settled<'a> {
    pub binding: OldBinding<'a>,
    pub channel: Keyed<'a, (Segment, ChannelSettled<'a>)>,
    pub statics: Keyed<'a, (Segment, StaticsSettled<'a>)>,
    /// Each body's own facts as a whole, where one of them re-ran.
    pub facts: Keyed<'a, (Segment, ShapeFacts<'a>)>,
    /// Each callable body's born instance, where it re-ran.
    pub born: Keyed<'a, (Segment, StaticSolution<'a>)>,
    /// Each quote's code the value channel no longer types, beside its refusal.
    pub refusals: Keyed<'a, (Segment, &'a ShapeError<'a>)>,
    /// Each path re-resolved, by its chain root's segment and its level.
    pub resolutions: Keyed<'a, ((Segment, usize), Resolution<'a>)>,
    /// How many facts each pass re-ran: the type channel's, then the value channel's.
    pub reran: (u32, u32),
}

impl<'a> Settled<'a> {
    /// A resumption under `binding` that settled nothing yet.
    pub fn empty(binding: OldBinding<'a>) -> Self {
        Settled {
            binding,
            channel: Keyed::from_sorted(&[]),
            statics: Keyed::from_sorted(&[]),
            facts: Keyed::from_sorted(&[]),
            born: Keyed::from_sorted(&[]),
            refusals: Keyed::from_sorted(&[]),
            resolutions: Keyed::from_sorted(&[]),
            reran: (0, 0),
        }
    }
}

/// A resumption a pass runs: the binding it resumes under, the resumptions it extends, newest
/// first, and whether it re-runs every fact rather than the ones the binding wakes — what the
/// law that quiescence is exact compares it with.
#[derive(Clone, Copy)]
pub struct Resumption<'held, 'a> {
    pub binding: OldBinding<'a>,
    pub base: &'held [&'a Settled<'a>],
    pub everything: bool,
}

impl<'held, 'a> Resumption<'held, 'a> {
    /// The type `unknown` is bound to.
    pub fn bound(self, unknown: Unknown) -> Option<KType> {
        self.binding.bound(unknown)
    }

    /// Whether `frontier` waits on an unknown this resumption binds and the ones it extends do
    /// not: whether the fact re-runs.
    pub fn wakes(self, frontier: Frontier<'_>) -> bool {
        let prior = self
            .base
            .first()
            .map_or(OldBinding::EMPTY, |base| base.binding);
        self.everything
            || (frontier.unknowns().iter())
                .any(|unknown| self.bound(*unknown).is_some() && prior.bound(*unknown).is_none())
    }

    /// The newest of `found` over the resumptions this one extends.
    fn newest<T>(self, found: impl Fn(&Settled<'a>) -> Option<T>) -> Option<T> {
        self.base.iter().find_map(|settled| found(settled))
    }

    pub fn channel(self, segment: Segment) -> impl Iterator<Item = &'a ChannelSettled<'a>> + 'held {
        (self.base.iter()).filter_map(move |settled| {
            let Settled { channel, .. } = **settled;
            channel.get(segment).map(|(_, held)| held)
        })
    }

    pub fn statics(self, segment: Segment) -> Option<StaticsSettled<'a>> {
        self.newest(|settled| settled.statics.get(segment).map(|(_, held)| *held))
    }

    pub fn facts(self, segment: Segment) -> Option<ShapeFacts<'a>> {
        self.newest(|settled| settled.facts.get(segment).map(|(_, held)| *held))
    }

    pub fn born(self, segment: Segment) -> Option<StaticSolution<'a>> {
        self.newest(|settled| settled.born.get(segment).map(|(_, held)| *held))
    }

    pub fn refusal(self, segment: Segment) -> Option<&'a ShapeError<'a>> {
        self.newest(|settled| settled.refusals.get(segment).map(|(_, held)| *held))
    }

    pub fn resolution(self, root: Segment, level: usize) -> Option<Resolution<'a>> {
        self.newest(|settled| {
            settled
                .resolutions
                .get((root, level))
                .map(|(_, held)| *held)
        })
    }
}

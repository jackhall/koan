//! A **build layer**: one walk's facts, the base of a load's or a composition's records, resting in
//! program storage for the load's and in a composition's storage for its own. It keeps the names
//! it bound, the records of the units the walk ran and the closings it wrote, the index of the
//! facts those records hold, and every closed tree they computed by its identity, each found as
//! the load finds its own ([`records`](super::records)). Every body's folds are the load's.
//!
//! A composition's layer rests nothing in the agent tier: beside its records it keeps a [`Term`]
//! of each type they name that the root and the agent tier do not hold, which a frame under it
//! mints. It keeps the refusal its code reached, its types termed as its records' would be, so a
//! frame born under it faults as the load would. A step reads a layer only through
//! [`Facts`](super::Facts), at its own lifetime. See
//! [build/README.md § The store](../build/README.md#the-store).

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::type_lattice::{DeclaredType, Parametric, Term, TypeDigest};
use crate::values::ContentDigest;

use super::derived::Derived;
use super::facts::{Frontier, UnitId};
use super::keyed::Keyed;
use super::records::{Closing, Held, UnitFacts, UnitRecord};
use super::shape::{Segment, ShapeError};
use super::shapes::Shapes;
use super::tree::{Tree, closed_trees};

/// One walk's facts: a load's or a composition's base.
#[derive(Clone, Copy)]
pub struct BuildLayer<'a> {
    /// Each unit the walk ran, beside its record and the frontier it gathered, and each it marked
    /// unreached.
    pub(super) units: Keyed<'a, (UnitId, UnitRecord<'a>)>,
    /// Each owning body the walk closed.
    pub(super) closings: Keyed<'a, (Segment, (&'a Closing<'a>, Frontier<'a>))>,
    /// Which of `units` holds each site, through and path fact: the facts the shape cannot place.
    pub(super) index: Keyed<'a, (Held, u32)>,
    /// Each closed tree `units` computed, a subtree included, by its identity.
    pub(super) trees: Keyed<'a, (ContentDigest, &'a Tree<'a>)>,
    /// Each derived name the walk met: where a reader finds the term of a name its records hold.
    derived: Derived<'a>,
    refusal: Option<&'a ShapeError<'a>>,
    /// By the type's digest: empty under the root.
    terms: Keyed<'a, (TypeDigest, Term<'a>)>,
}

impl<'a> BuildLayer<'a> {
    /// A layer holding no record.
    pub const EMPTY: BuildLayer<'static> = BuildLayer {
        units: Keyed::EMPTY,
        closings: Keyed::EMPTY,
        index: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        derived: Derived::EMPTY,
        refusal: None,
        terms: Keyed::EMPTY,
    };

    /// The one lay: a walk's records over `shapes`, `units` and `closings` sorted by id, laid
    /// through `lay`.
    pub(super) fn laid(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        units: &[(UnitId, UnitRecord<'a>)],
        closings: &[(Segment, (&'a Closing<'a>, Frontier<'a>))],
    ) -> Self {
        let units = Keyed::from_sorted(collect(lay, units.iter().copied()));
        let mut held = BumpVec::new_in(scratch);
        for (at, (_, record)) in units.iter().enumerate() {
            if let UnitRecord::Typed(facts, _) = record {
                facts.held(&mut |key| held.push((key, at as u32)));
            }
        }
        held.sort_unstable();
        held.dedup();
        debug_assert!(
            held.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "each fact lies in one unit's record"
        );
        if cfg!(debug_assertions) {
            for (id, facts) in typed(units) {
                let placed = facts.typed.iter().all(|(segment, typed)| {
                    let mut slots = (typed.declared.iter().map(|(slot, _)| *slot))
                        .chain(typed.registered.iter().map(|(slot, _)| *slot))
                        .chain(typed.binders.iter().map(|(slot, _)| *slot))
                        .chain(typed.contents.iter().map(|(slot, _)| *slot));
                    slots.all(|slot| shapes.slot_unit(*segment, slot) == id)
                        && (typed.statements.iter())
                            .all(|(index, _)| shapes.running(*segment, *index) == id)
                }) && (facts.born.iter())
                    .all(|(body, _)| shapes.birth_unit(*body) == Some(id));
                assert!(
                    placed,
                    "a slot's, statement's or birth's fact lies in the unit the shape names"
                );
            }
        }
        BuildLayer {
            units,
            closings: Keyed::from_sorted(collect(lay, closings.iter().copied())),
            index: Keyed::from_sorted(collect(lay, held.iter().copied())),
            trees: computed_trees(lay, scratch, units),
            derived: Derived::EMPTY,
            refusal: None,
            terms: Keyed::EMPTY,
        }
    }

    /// These facts beside `refusal`, the refusal composed code reached beside its records.
    pub fn refusing(self, refusal: Option<&'a ShapeError<'a>>) -> Self {
        BuildLayer { refusal, ..self }
    }

    /// These facts beside `derived`, the derived names their walk met.
    pub(super) fn deriving(self, derived: Derived<'a>) -> Self {
        BuildLayer { derived, ..self }
    }

    /// The derived names this layer's walk met.
    pub fn derived(&self) -> Derived<'a> {
        self.derived
    }

    /// These facts beside `terms`, a term of each type their records name past the agent tier,
    /// by the type's digest.
    pub fn termed(self, terms: Keyed<'a, (TypeDigest, Term<'a>)>) -> Self {
        BuildLayer { terms, ..self }
    }

    /// Hand `each` every type this layer's records name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for (_, record) in self.units.iter() {
            if let UnitRecord::Typed(facts, frontier) = record {
                facts.each_type(each);
                frontier.each_type(each);
            }
        }
        for (_, (closing, frontier)) in self.closings.iter() {
            closing.each_type(each);
            frontier.each_type(each);
        }
        self.derived.each_type(each);
    }

    /// A term of each type its records name that the root and the agent tier do not hold, by the
    /// type's digest: empty under the root.
    pub fn terms(&self) -> Keyed<'a, (TypeDigest, Term<'a>)> {
        self.terms
    }

    /// The refusal its walk reached, if it reached one.
    pub fn refusal(&self) -> Option<&'a ShapeError<'a>> {
        self.refusal
    }

    /// The units it marked unreached, by id.
    #[cfg(test)]
    pub(crate) fn unreached_units(&self) -> impl Iterator<Item = UnitId> + '_ {
        (self.units.iter()).filter_map(|(unit, record)| record.typed().is_none().then_some(*unit))
    }

    /// Its own record of `unit`, or its mark that its walk did not reach `unit`.
    pub(crate) fn unit(&self, unit: UnitId) -> Option<UnitRecord<'a>> {
        self.units.get(unit).map(|(_, record)| *record)
    }

    /// Its own closing record of the owning body `body`, beside its frontier.
    pub(super) fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)> {
        self.closings.get(body).map(|(_, record)| *record)
    }

    /// Its own record holding `held`, beside its unit.
    pub(super) fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)> {
        let (_, at) = self.index.get(held)?;
        let (unit, record) = self.units[*at as usize];
        let (facts, _) = record.typed().expect("the index points at reached units");
        Some((unit, facts))
    }

    /// The closed tree whose identity is `digest`, among those its own records computed.
    pub(super) fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.trees.get(digest).map(|(_, tree)| *tree)
    }
}

/// Each closed tree `units` computed, a subtree included, by its identity, laid through `lay`.
fn computed_trees<'a>(
    lay: impl Lays<'a>,
    scratch: BumpAllocator<'_>,
    units: Keyed<'a, (UnitId, UnitRecord<'a>)>,
) -> Keyed<'a, (ContentDigest, &'a Tree<'a>)> {
    let computed = typed(units)
        .flat_map(|(_, facts)| facts.typed.iter())
        .flat_map(|(_, typed)| {
            (typed.contents.iter().map(|(_, tree)| tree))
                .chain(typed.trees.iter().map(|(_, tree)| *tree))
        });
    closed_trees(lay, scratch, computed)
}

/// The units of `units` a walk reached, each beside its facts.
pub(super) fn typed<'a>(
    units: Keyed<'a, (UnitId, UnitRecord<'a>)>,
) -> impl Iterator<Item = (UnitId, &'a UnitFacts<'a>)> + 'a {
    (units.entries().iter()).filter_map(|(unit, record)| Some((*unit, record.typed()?.0)))
}

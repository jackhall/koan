//! A **settlement's facts**: what one walk rooted at a body decided under one binding, over its
//! parent's. It keeps the binding, the records of the units the binding woke and the closings the
//! walk wrote, the index of the facts those records hold, and every closed tree they computed by
//! its identity, each found as the load finds its own ([`records`](super::records)). The body's
//! folds stay the load's, so a read of one falls to the load.
//!
//! A settlement whose walk reached a refusal keeps the refusal's wording instead of records, so a
//! frame born under it faults as the load would, and a second ask refuses alike. The arena owns
//! each settlement beside its parent; a step reads one only through [`Facts`](super::Facts), at
//! its own lifetime.
//!
//! An [`Activated`] layer is what a frame decides in its own activation at one binding event: the
//! walk rooted at its body under the names the event bound, over everything the frame read below.
//! The [records copier](super::relay) lays it in the frame's region, so it points at nothing an
//! arena node holds; it binds only the names its event bound, and a read of any other falls below.

use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, resident};

use super::facts::{Binding, Bound, Unknown};
use super::records::Records;
use super::relay::Relay;
use super::shapes::Shapes;
use super::writes::KeyedWrites;

/// One settlement's facts over its parent's, the root's where it has none.
#[derive(Clone, Copy)]
pub struct Settlement<'a> {
    parent: Option<&'a Settlement<'a>>,
    records: Records<'a>,
    refusal: Option<&'a str>,
}

impl<'a> Settlement<'a> {
    /// The facts a walk over `shapes` under `binding` wrote into `writes`, over `parent`'s, laid
    /// through `lay`.
    pub fn of(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        parent: Option<&'a Settlement<'a>>,
        binding: Binding<'a>,
        writes: &KeyedWrites<'a, '_>,
    ) -> Self {
        let (units, closings) = writes.sorted(scratch);
        Settlement {
            parent,
            records: Records::of(lay, scratch, shapes, None, binding, &units, &closings),
            refusal: None,
        }
    }

    /// A settlement over `parent`'s binding that holds no fact of its own.
    pub fn empty(parent: Option<&'a Settlement<'a>>) -> Self {
        Settlement {
            parent,
            records: Records::bare(parent.map_or(Binding::EMPTY, |parent| parent.binding())),
            refusal: None,
        }
    }

    /// A settlement whose walk under `binding` refused, worded `refusal`: it holds no record.
    pub fn refused(
        parent: Option<&'a Settlement<'a>>,
        binding: Binding<'a>,
        refusal: &'a str,
    ) -> Self {
        Settlement {
            parent,
            records: Records::bare(binding),
            refusal: Some(refusal),
        }
    }

    /// The wording of the refusal its walk reached, if it reached one.
    pub fn refusal(&self) -> Option<&'a str> {
        self.refusal
    }

    /// The binding it settled under: its parent's and more.
    pub fn binding(&self) -> Binding<'a> {
        self.records.binding()
    }

    /// Hand `each` every type its own records name.
    #[cfg(test)]
    pub fn each_type(
        &self,
        each: &mut dyn FnMut(crate::type_lattice::DeclaredType<crate::type_lattice::Parametric>),
    ) {
        self.records.each_type(each);
    }

    pub(super) fn parent(&self) -> Option<&'a Settlement<'a>> {
        self.parent
    }

    pub(super) fn records(&self) -> &Records<'a> {
        &self.records
    }
}

/// One binding event's facts in a frame's activation, over the event's below it.
#[derive(Clone, Copy)]
pub struct Activated<'a> {
    below: Option<&'a Activated<'a>>,
    /// Bound to the names this event bound alone.
    records: Records<'a>,
}

impl<'a> Activated<'a> {
    /// The facts a walk over `shapes` wrote into `writes` under `binding`, which binds `own` over
    /// what the layers beneath bind, laid again through `lay` over `below`.
    pub fn of(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        below: Option<&'a Activated<'a>>,
        binding: Binding<'_>,
        own: &[(Unknown, Bound<'_>)],
        writes: &KeyedWrites<'_, '_>,
    ) -> Self {
        let mut relay = Relay::new(lay, scratch, binding);
        let (units, closings) = writes.sorted(scratch);
        let mut laid = BumpVec::with_capacity_in(units.len(), scratch);
        for (unit, (facts, frontier)) in units.iter() {
            let frontier = relay.frontier(*frontier);
            laid.push((*unit, (resident(lay, relay.unit(facts)), frontier)));
        }
        let mut closed = BumpVec::with_capacity_in(closings.len(), scratch);
        for (body, (closing, frontier)) in closings.iter() {
            let frontier = relay.frontier(*frontier);
            closed.push((*body, (resident(lay, relay.closing(closing)), frontier)));
        }
        let mut bound = BumpVec::with_capacity_in(own.len(), scratch);
        bound.extend(own.iter().map(|(unknown, at)| (*unknown, relay.bound(at))));
        Activated {
            below,
            records: Records::of(
                lay,
                scratch,
                shapes,
                None,
                Binding::laid(lay, &mut bound),
                &laid,
                &closed,
            ),
        }
    }

    /// Hand `each` every type the records of this event and every one below it name.
    pub fn each_type(
        &self,
        each: &mut dyn FnMut(crate::type_lattice::DeclaredType<crate::type_lattice::Parametric>),
    ) {
        let mut at = Some(self);
        while let Some(layer) = at {
            layer.records.each_type(each);
            at = layer.below;
        }
    }

    /// This event and every one below it laid again through `lay`: what a function value carries
    /// out of the region its closure was born in, once the crossing has made every type they name
    /// resolve there.
    pub fn copied<'to, L: Lays<'to>>(&self, lay: L) -> &'to Activated<'to> {
        let scratch = Bump::new();
        let mut layers = BumpVec::new_in(&scratch);
        let mut at = Some(self);
        while let Some(layer) = at {
            layers.push(layer);
            at = layer.below;
        }
        // One copier for every layer: a frontier two layers share is laid once.
        let mut relay = Relay::new(lay, &scratch, Binding::EMPTY);
        let mut below = None;
        for layer in layers.iter().rev() {
            let records = layer.records.relaid(lay, &scratch, &mut relay);
            below = Some(resident(lay, Activated { below, records }));
        }
        below.expect("an event is a layer")
    }

    /// What [`copied`](Self::copied) lays for this event and every one below it: each layer and
    /// its records' tables, by entry. A record's own slices are not counted.
    pub fn weight(&self) -> crate::values::Weight {
        let mut weight = crate::values::Weight::flat::<()>();
        let mut at = Some(self);
        while let Some(layer) = at {
            weight = weight
                .plus(crate::values::Weight::flat::<Activated<'_>>())
                .plus(layer.records.weight());
            at = layer.below;
        }
        weight
    }

    /// The event below this one, if any.
    pub(super) fn below(&self) -> Option<&'a Activated<'a>> {
        self.below
    }

    pub(super) fn records(&self) -> &Records<'a> {
        &self.records
    }
}

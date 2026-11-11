//! A **build layer**: one body's facts decided under one binding, over the layer below it, wherever
//! it rests — program storage for the load's, a depot slot's bump for a settlement a miss builds,
//! a frame's region for what an activation decides. It keeps the names it bound alone, the
//! records of the units the binding woke and the closings the walk wrote, the index of the facts
//! those records hold, and every closed tree they computed by its identity, each found as the load
//! finds its own ([`records`](super::records)). A read of a name it does not bind, or of a unit it
//! did not re-run, falls to its parent, then to the chain's base. Every body's folds stay the
//! load's.
//!
//! A layer under a composition rests nothing in the agent tier: beside its records it keeps a
//! [`Term`] of each type they name that the root and the agent tier do not hold, which a frame
//! under it mints, as one under the composition mints the composition's.
//!
//! Beside its facts a layer carries the request ([`SiteRequest`]) of each call site of its body
//! that passes its callee's keys from load content or the body's own slots: what a call at that
//! site hands the drain in place of a key computed from its values. The base carries each site
//! passing load content alone.
//!
//! A layer whose walk reached a refusal keeps the refusal instead of records, its types rested or
//! termed as its records' would be, so a frame born under it faults as the load would, and a
//! second ask refuses alike. A step reads layers only through [`Facts`](super::Facts), at its own
//! lifetime.
//!
//! What a frame decides in its own activation is laid in its region by the
//! [records copier](super::relay), so it points at nothing a depot slot holds. See
//! [build/README.md § The store](../build/README.md#the-store).

use crate::memory::{Bump, BumpAllocator, BumpVec, Lays, collect, resident};
use crate::type_lattice::{DeclaredType, KType, Parametric, Term, TypeDigest};
use crate::values::{ContentDigest, Weight};

use super::admission::Admission;
use super::closed::ClosedTypes;
use super::facts::{Binding, Bound, Frontier, UnitId, Unknown};
use super::keyed::Keyed;
use super::records::{Closing, Held, UnitFacts};
use super::relay::Relay;
use super::shape::{Segment, ShapeError, Site, Slot};
use super::shapes::Shapes;
use super::stamp::SiteRequest;
use super::tree::{Identity, Tree, closed_trees};
use super::writes::KeyedWrites;

/// What a witnessed layer keeps for the bindings it admits: a layer a witnessing walk laid, whose
/// records are rigid over the names it witnessed and beside their questions, and over which each
/// binding that keeps them lays its own layer.
#[derive(Clone, Copy, Debug)]
pub struct Witnessing<'a> {
    /// The names it witnessed: what a closing walk over it checks.
    pub names: &'a [Unknown],
    /// The table keys a binding it admits closes: the first closing walk's, which no binding that
    /// keeps every question changes, and the types its own closing records name.
    pub keys: &'a [DeclaredType<Parametric>],
    /// Whether a binding keeping every question still walks: the first closing walk over it re-ran
    /// a unit, or one of its closing records' frontiers woke under that walk.
    pub holds: bool,
    /// Its [`witnessed_digest`](super::memo::witnessed_digest): what the spine of a cell over it
    /// carries in its memo key.
    pub digest: ContentDigest,
}

/// One body's facts decided under one binding, over its parent's: the base's, where it has none.
#[derive(Clone, Copy)]
pub struct BuildLayer<'a> {
    /// The layer below in the chain: `None` at a base, at the bottom of a depot chain, and at the
    /// bottom of a frame's decided layers.
    parent: Option<&'a BuildLayer<'a>>,
    /// The names this layer bound alone; a load's base binds nothing.
    pub(super) binding: Binding<'a>,
    /// The conditions on a binding, up to its event, under which this layer's decision holds.
    admission: Admission<'a>,
    /// The closed type the chain's binding gives each parametric type the records this layer's
    /// walk absorbed name over its binding: what a reader closes a shared record's facts by.
    closed: ClosedTypes<'a>,
    /// Whether this layer's table or one below it holds a type: a reader with no such layer on
    /// its chain closes nothing, without walking the chain ([`flagged`](Self::flagged)).
    closes: bool,
    /// What it keeps for the bindings it admits, where a witnessing walk laid it.
    witnessing: Option<Witnessing<'a>>,
    /// Each unit the walk ran, beside its record and the frontier it gathered.
    pub(super) units: Keyed<'a, (UnitId, (&'a UnitFacts<'a>, Frontier<'a>))>,
    /// Each owning body the walk closed.
    pub(super) closings: Keyed<'a, (Segment, (&'a Closing<'a>, Frontier<'a>))>,
    /// Which of `units` holds each site, through and path fact: the facts the shape cannot place.
    pub(super) index: Keyed<'a, (Held, u32)>,
    /// Each closed tree `units` computed, a subtree included, by its identity.
    pub(super) trees: Keyed<'a, (ContentDigest, &'a Tree<'a>)>,
    refusal: Option<&'a ShapeError<'a>>,
    /// By the type's digest: empty under the root and in a region.
    terms: Keyed<'a, (TypeDigest, Term<'a>)>,
    /// The request of each call site whose slot map passes, at every keyed name of its callee, load
    /// content or a slot of this layer's body, by the site's shape and site: on a base, each site
    /// passing load content alone. Empty in a region.
    requests: Keyed<'a, ((Segment, Site), SiteRequest<'a>)>,
}

impl<'a> BuildLayer<'a> {
    /// A layer binding nothing and holding no record.
    pub const EMPTY: BuildLayer<'static> = BuildLayer {
        parent: None,
        binding: Binding::EMPTY,
        admission: Admission::EMPTY,
        closed: ClosedTypes::EMPTY,
        closes: false,
        witnessing: None,
        units: Keyed::EMPTY,
        closings: Keyed::EMPTY,
        index: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        refusal: None,
        terms: Keyed::EMPTY,
        requests: Keyed::EMPTY,
    };

    /// The facts a walk over `shapes` wrote into `writes`, over `parent`, laid through `lay`: it
    /// binds `own`, the names it bound alone, and closes what it absorbed by `closed`.
    pub fn of(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        parent: Option<&'a BuildLayer<'a>>,
        own: Binding<'a>,
        writes: &KeyedWrites<'a, '_>,
    ) -> Self {
        let (units, closings) = writes.sorted(scratch);
        BuildLayer::laid(lay, scratch, shapes, parent, own, &units, &closings)
            .with_closed(writes.closed())
    }

    /// The one lay: a walk's records over `shapes`, `units` and `closings` sorted by id, laid
    /// through `lay` over `parent`, binding `own`.
    pub(super) fn laid(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        parent: Option<&'a BuildLayer<'a>>,
        own: Binding<'a>,
        units: &[(UnitId, (&'a UnitFacts<'a>, Frontier<'a>))],
        closings: &[(Segment, (&'a Closing<'a>, Frontier<'a>))],
    ) -> Self {
        let units = Keyed::from_sorted(collect(lay, units.iter().copied()));
        let mut held = BumpVec::new_in(scratch);
        for (at, (_, (facts, _))) in units.iter().enumerate() {
            facts.held(&mut |key| held.push((key, at as u32)));
        }
        held.sort_unstable();
        // A class many units resolve a path to is found through the first.
        held.dedup_by(|later, first| {
            later.0 == first.0 && (later == first || matches!(later.0, Held::Class(..)))
        });
        debug_assert!(
            held.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "each fact lies in one unit's record"
        );
        if cfg!(debug_assertions) {
            for (id, (facts, _)) in units.iter() {
                let placed = facts.typed.iter().all(|(segment, typed)| {
                    let mut slots = (typed.declared.iter().map(|(slot, _)| *slot))
                        .chain(typed.registered.iter().map(|(slot, _)| *slot))
                        .chain(typed.binders.iter().map(|(slot, _)| *slot))
                        .chain(typed.contents.iter().map(|(slot, _)| *slot));
                    slots.all(|slot| shapes.slot_unit(*segment, slot) == *id)
                        && (typed.statements.iter())
                            .all(|(index, _)| shapes.running(*segment, *index) == *id)
                }) && (facts.born.iter())
                    .all(|(body, _)| shapes.birth_unit(*body) == Some(*id));
                assert!(
                    placed,
                    "a slot's, statement's or birth's fact lies in the unit the shape names"
                );
            }
        }
        BuildLayer {
            parent,
            binding: own,
            admission: Admission::binding(lay, own),
            closed: ClosedTypes::EMPTY,
            closes: false,
            witnessing: None,
            units,
            closings: Keyed::from_sorted(collect(lay, closings.iter().copied())),
            index: Keyed::from_sorted(collect(lay, held.iter().copied())),
            trees: computed_trees(lay, scratch, units),
            refusal: None,
            terms: Keyed::EMPTY,
            requests: Keyed::EMPTY,
        }
        .flagged()
    }

    /// A binding's layer over `parent`, a witnessed layer the binding keeps every question of and
    /// re-runs nothing over: it binds `own`, admits by `admission`, and holds no record, only
    /// `closed`, its table of closed types. No walk lays it.
    pub fn kept(
        parent: &'a BuildLayer<'a>,
        own: Binding<'a>,
        admission: Admission<'a>,
        closed: ClosedTypes<'a>,
    ) -> Self {
        BuildLayer {
            parent: Some(parent),
            binding: own,
            admission,
            closed,
            ..BuildLayer::EMPTY
        }
        .flagged()
    }

    /// These facts as a witnessed layer: admitting a binding that keeps their questions by
    /// `admission`, beside `witnessing`.
    pub fn witnessed(self, admission: Admission<'a>, witnessing: Witnessing<'a>) -> Self {
        BuildLayer {
            admission,
            witnessing: Some(witnessing),
            ..self
        }
    }

    /// What it keeps for the bindings it admits, where a witnessing walk laid it.
    pub fn witnessing(&self) -> Option<Witnessing<'a>> {
        self.witnessing
    }

    /// A layer over `parent` that binds nothing and holds no fact of its own.
    pub fn empty(parent: Option<&'a BuildLayer<'a>>) -> Self {
        BuildLayer {
            parent,
            ..BuildLayer::EMPTY
        }
        .flagged()
    }

    /// A layer whose walk under `own` reached `refusal`: it holds no record.
    pub fn refused(
        lay: impl Lays<'a>,
        parent: Option<&'a BuildLayer<'a>>,
        own: Binding<'a>,
        refusal: &'a ShapeError<'a>,
    ) -> Self {
        BuildLayer {
            parent,
            binding: own,
            admission: Admission::binding(lay, own),
            refusal: Some(refusal),
            ..BuildLayer::EMPTY
        }
        .flagged()
    }

    /// These facts beside `requests`, the request of each covered call site, by shape and site.
    pub fn requesting(self, requests: Keyed<'a, ((Segment, Site), SiteRequest<'a>)>) -> Self {
        BuildLayer { requests, ..self }
    }

    /// The request of the call site `site` of the shape `at`, if this layer carries one.
    pub fn request(&self, at: Segment, site: Site) -> Option<&'a SiteRequest<'a>> {
        self.requests.get((at, site)).map(|(_, request)| request)
    }

    /// These facts beside `refusal`, the refusal composed code reached beside its records.
    pub fn refusing(self, refusal: Option<&'a ShapeError<'a>>) -> Self {
        BuildLayer { refusal, ..self }
    }

    /// These facts beside `closed`, the table of closed types for what their walk absorbed.
    pub(super) fn with_closed(self, closed: ClosedTypes<'a>) -> Self {
        BuildLayer { closed, ..self }.flagged()
    }

    /// This layer with [`closes`](Self::closes) set from its own table and its parent's: every
    /// constructor that sets either ends here.
    fn flagged(self) -> Self {
        let closes = !self.closed.is_empty() || self.parent.is_some_and(|parent| parent.closes);
        BuildLayer { closes, ..self }
    }

    /// These facts beside `terms`, a term of each type their records name past the agent tier,
    /// by the type's digest.
    pub fn termed(self, terms: Keyed<'a, (TypeDigest, Term<'a>)>) -> Self {
        BuildLayer { terms, ..self }
    }

    /// The facts a binding event's walk over `shapes` wrote into `writes` under `binding`, which
    /// binds `own` over what the layers beneath bind, laid again through `lay` over `parent`: each
    /// record copied through one [`Relay`], since a region never points into a depot slot's bump.
    #[allow(clippy::too_many_arguments)]
    pub fn relaid(
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        parent: Option<&'a BuildLayer<'a>>,
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
        let own = Binding::laid(lay, &mut bound);
        BuildLayer::laid(lay, scratch, shapes, parent, own, &laid, &closed)
            .with_closed(writes.closed().copied(lay))
    }

    /// These facts admitting a binding by `admission`.
    pub fn admitting(self, admission: Admission<'a>) -> Self {
        BuildLayer { admission, ..self }
    }

    /// This layer alone over `parent`: a decision a memo kept, as an event over the cell's below.
    pub fn over(self, parent: Option<&'a BuildLayer<'a>>) -> Self {
        BuildLayer { parent, ..self }.flagged()
    }

    /// This layer over `parent`: itself where it already lies there, else laid over it through
    /// `lay`. A witnessed layer read over the layers it was laid over costs nothing.
    pub fn resting_over(
        &'a self,
        lay: impl Lays<'a>,
        parent: Option<&'a BuildLayer<'a>>,
    ) -> &'a BuildLayer<'a> {
        match (self.parent, parent) {
            (Some(below), Some(parent)) if std::ptr::addr_eq(below, parent) => self,
            (None, None) => self,
            _ => resident(lay, self.over(parent)),
        }
    }

    /// This layer alone, over `parent`, laid again through `lay` by `relay`, its trees gathered in
    /// `scratch`: what a copy of a frame's decided layer into another region holds, and what an
    /// activation lays in its frame's region of a layer its walks laid in scratch.
    pub(crate) fn relayed<'to, L: Lays<'to>>(
        &self,
        lay: L,
        scratch: BumpAllocator<'_>,
        relay: &mut Relay<'to, '_, '_, 'a, L>,
        parent: Option<&'to BuildLayer<'to>>,
    ) -> BuildLayer<'to> {
        debug_assert!(
            self.refusal.is_none()
                && self.terms.entries().is_empty()
                && self.requests.entries().is_empty(),
            "a region layer holds no refusal, no term and no request"
        );
        let mut units = BumpVec::with_capacity_in(self.units.len(), scratch);
        for (unit, (facts, frontier)) in self.units.iter() {
            let frontier = relay.frontier(*frontier);
            units.push((*unit, (resident(lay, relay.unit(facts)), frontier)));
        }
        let mut closings = BumpVec::with_capacity_in(self.closings.len(), scratch);
        for (body, (closing, frontier)) in self.closings.iter() {
            let frontier = relay.frontier(*frontier);
            closings.push((*body, (resident(lay, relay.closing(closing)), frontier)));
        }
        let mut bound = BumpVec::with_capacity_in(self.binding.entries().len(), scratch);
        (bound).extend(
            (self.binding.entries().iter()).map(|(unknown, at)| (*unknown, relay.bound(at))),
        );
        let units = Keyed::from_sorted(collect(lay, units.iter().copied()));
        let binding = Binding::laid(lay, &mut bound);
        BuildLayer {
            parent,
            binding,
            admission: self.admission.relaid(lay, relay),
            closed: self.closed.copied(lay),
            closes: false,
            witnessing: self.witnessing.map(|witnessing| Witnessing {
                names: collect(lay, witnessing.names.iter().copied()),
                keys: collect(lay, witnessing.keys.iter().copied()),
                holds: witnessing.holds,
                digest: witnessing.digest,
            }),
            units,
            closings: Keyed::from_sorted(collect(lay, closings.iter().copied())),
            index: Keyed::from_sorted(collect(lay, self.index.iter().copied())),
            trees: computed_trees(lay, scratch, units),
            refusal: None,
            terms: Keyed::EMPTY,
            requests: Keyed::EMPTY,
        }
        .flagged()
    }

    /// This layer and every one below it laid again through `lay`: what a function value carries
    /// out of the region its closure was born in, once the crossing has made every type they name
    /// resolve there.
    pub fn copied<'to, L: Lays<'to>>(&self, lay: L) -> &'to BuildLayer<'to> {
        let scratch = Bump::new();
        let mut layers: BumpVec<'_, &BuildLayer<'_>> = BumpVec::new_in(&scratch);
        layers.extend(self.chain());
        // One copier for every layer: a frontier two layers share is laid once.
        let mut relay = Relay::new(lay, &scratch, Binding::EMPTY);
        let mut parent = None;
        for layer in layers.iter().rev() {
            let laid = layer.relayed(lay, &scratch, &mut relay, parent);
            parent = Some(resident(lay, laid));
        }
        parent.expect("a chain holds its own layer")
    }

    /// What [`copied`](Self::copied) lays for this layer and every one below it: each layer and
    /// its tables, by entry. A record's own slices are not counted.
    pub fn weight(&self) -> Weight {
        let mut weight = Weight::flat::<()>();
        for layer in self.chain() {
            let units = Weight::run::<(UnitId, (&UnitFacts<'_>, Frontier<'_>))>(layer.units.len())
                .plus(Weight::run::<UnitFacts<'_>>(layer.units.len()));
            let closings =
                Weight::run::<(Segment, (&Closing<'_>, Frontier<'_>))>(layer.closings.len())
                    .plus(Weight::run::<Closing<'_>>(layer.closings.len()));
            weight = weight
                .plus(Weight::flat::<BuildLayer<'_>>())
                .plus(units)
                .plus(closings)
                .plus(Weight::run::<(Unknown, Bound<'_>)>(
                    layer.binding.entries().len(),
                ))
                .plus(Weight::run::<(Held, u32)>(layer.index.len()))
                .plus(
                    Weight::run::<(DeclaredType<Parametric>, DeclaredType<KType>)>(
                        layer.closed.len(),
                    ),
                );
        }
        weight
    }

    /// Hand `each` every type the records of this layer and every one below it name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        self.chain().for_each(|layer| layer.each_own_type(each));
    }

    /// Hand `each` every type this layer's own records and requests name.
    pub(crate) fn each_own_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for (_, (facts, frontier)) in self.units.iter() {
            facts.each_type(each);
            frontier.each_type(each);
        }
        for (_, (closing, frontier)) in self.closings.iter() {
            closing.each_type(each);
            frontier.each_type(each);
        }
        for (_, request) in self.requests.iter() {
            request.each_type(each);
        }
        self.closed.each_type(each);
        (self.admission).each_kept(&mut |answered| answered.question.each_type(each));
        for key in self
            .witnessing
            .iter()
            .flat_map(|witnessing| witnessing.keys)
        {
            each(*key);
        }
    }

    /// This layer, then each below it.
    pub(super) fn chain(&self) -> impl Iterator<Item = &BuildLayer<'a>> {
        std::iter::successors(Some(self), |layer| layer.parent)
    }

    /// A term of each type its records name that the root and the agent tier do not hold, by the
    /// type's digest: empty under the root and in a region.
    pub fn terms(&self) -> Keyed<'a, (TypeDigest, Term<'a>)> {
        self.terms
    }

    /// The refusal its walk reached, if it reached one.
    pub fn refusal(&self) -> Option<&'a ShapeError<'a>> {
        self.refusal
    }

    /// The conditions on a binding under which its decision holds.
    pub fn admission(&self) -> Admission<'a> {
        self.admission
    }

    /// The closed types it gives what the records its walk absorbed name.
    pub fn closed(&self) -> ClosedTypes<'a> {
        self.closed
    }

    /// Whether this layer's table of closed types, or one below it, holds a type.
    pub fn closes(&self) -> bool {
        self.closes
    }

    /// The names it bound alone: a read of any other falls to its parent.
    pub fn binding(&self) -> Binding<'a> {
        self.binding
    }

    /// The layer below it, if any.
    pub fn parent(&self) -> Option<&'a BuildLayer<'a>> {
        self.parent
    }

    /// Whether it is a binding's layer over a witnessed one: what a spine leaves out.
    pub fn over_witnessed(&self) -> bool {
        (self.parent).is_some_and(|below| below.witnessing.is_some())
    }

    /// Every name a witnessed layer of the chain from `top` down witnessed, sorted, each once: what
    /// an event witnesses beside its own type names, its cell's earlier events' and its
    /// enclosers'.
    pub fn witnessed_names<'x>(
        scratch: BumpAllocator<'x>,
        top: Option<&BuildLayer<'a>>,
    ) -> BumpVec<'x, Unknown> {
        let mut names = BumpVec::new_in(scratch);
        for layer in std::iter::successors(top, |layer| layer.parent) {
            names.extend((layer.witnessing.iter()).flat_map(|witnessing| witnessing.names));
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Its own record of `unit`, where its walk ran it.
    #[cfg(test)]
    pub(crate) fn unit_of(&self, unit: UnitId) -> Option<&'a UnitFacts<'a>> {
        self.unit(unit).map(|(facts, _)| facts)
    }

    /// The units it holds a record of, by id.
    #[cfg(test)]
    pub(crate) fn own_units(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.units.iter().map(|(unit, _)| *unit)
    }

    /// Its own record of `unit`, beside its frontier.
    pub(crate) fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        self.units.get(unit).map(|(_, record)| *record)
    }

    /// Its own closing record of the owning body `body`, beside its frontier.
    pub(super) fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)> {
        self.closings.get(body).map(|(_, record)| *record)
    }

    /// Its own record holding `held`, beside its unit.
    pub(super) fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)> {
        let (_, at) = self.index.get(held)?;
        let (unit, (facts, _)) = self.units[*at as usize];
        Some((unit, facts))
    }

    /// Hand `each` every module binder of `segment` its own records say holds a module of
    /// `identity`'s tree, beside its unit.
    pub(super) fn holders(
        &self,
        segment: Segment,
        identity: Identity,
        each: &mut dyn FnMut(UnitId, Slot),
    ) {
        let entries = self.index.entries();
        let from =
            entries.partition_point(|(held, _)| *held < Held::Holder(segment, identity, Slot(0)));
        for (held, at) in &entries[from..] {
            let Held::Holder(body, holding, slot) = *held else {
                break;
            };
            if body != segment || holding != identity {
                break;
            }
            each(self.units[*at as usize].0, slot);
        }
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
    units: Keyed<'a, (UnitId, (&'a UnitFacts<'a>, Frontier<'a>))>,
) -> Keyed<'a, (ContentDigest, &'a Tree<'a>)> {
    let computed = (units.iter())
        .flat_map(|(_, (facts, _))| facts.typed.iter())
        .flat_map(|(_, typed)| {
            (typed.contents.iter().map(|(_, tree)| tree))
                .chain(typed.trees.iter().map(|(_, tree)| *tree))
        });
    closed_trees(lay, scratch, computed)
}

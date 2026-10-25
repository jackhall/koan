//! The **load's facts**, read through one set of readers over a source of records: [`LoadFacts`]
//! over one load's own, and [`Facts`], what a step reads through its door, over a cell's layers and
//! the load they rest on. Every reader takes the body's shape first, so no reader names where a
//! fact is kept. A shape names its body by its segment alone, so a reader may hand in a shape
//! borrowed for any lifetime.
//!
//! It reads the one walk's records: one [`UnitFacts`] per unit the walk ran, each beside the
//! frontier it gathered, one [`Closing`] per owning body, and the [`Folds`] the load lays per body
//! after the walk. Every record is `Copy`, names bodies by segment and holds no cell, so each is
//! covariant. A load may sit over a base: every read tries its own records, then its base's, and a
//! unit it re-ran replaces the base's record of it whole. The records index every closed tree their
//! units computed by its identity, read through [`LoadFacts::tree_of`].
//!
//! A `LoadFacts` is laid in program storage before the program is. A step reads it at its own
//! lifetime through the [`Facts`] its door builds, so nothing a cell parks points into it; a value
//! born of written code keeps what it holds of its body through [`Written`], since the load is
//! never evicted.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect, resident};
use crate::parse::{ExpressionPart, ProgramNode};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Term};
use crate::values::{Chain, ContentDigest, Weight};

use super::composition::Composition;
use super::facts::{Address, Binding, Bound, Frontier, UnitId, Unknown};
use super::keyed::Keyed;
use super::record_types::Named;
use super::relay::Relay;
use super::settlement_facts::{Activated, Settlement};
use super::shape::{
    BodyShape, Coordinate, Lands, Located, Position, Segment, ShapeError, Site, Slot, Target,
};
use super::shapes::Shapes;
use super::tree::{Leaf, NodeCapture, Tree, closed_trees};
use super::typed::{
    BodyClass, Caching, Elaboration, Handoff, InstanceRead, Mint, PathEntry, Pick, Points,
    Resolution, Route, Source, Static, StaticCallable, StaticInstance, StaticRegistered,
    StaticType, UseStatics, Variable, ViewSlots,
};

/// What a cell reads of a typed fact where it runs ([`Facts::at_run`]).
#[derive(Clone, Copy, Debug)]
pub enum AtRun<T> {
    /// The same at every run.
    Closed(T),
    /// Left to the run's own door: a contribution read off the argument's carried type, or a
    /// view's type over a tree the binding leaves open.
    Open,
    /// Refused at every run, as the load words it.
    Refused(Elaboration),
}

/// One unit's facts: everything the walk computed running it.
#[derive(Clone, Copy)]
pub struct UnitFacts<'a> {
    /// The unit's own body's, and the signature of each module body it ran, by segment.
    pub typed: Keyed<'a, (Segment, Typed<'a>)>,
    /// Each callable or quote's code born here, by its segment.
    pub born: Keyed<'a, (Segment, Born<'a>)>,
    /// Each path entry this unit resolved first on its chain, by its chain root and level, beside
    /// its resolution.
    pub paths: Keyed<'a, ((Segment, usize), (PathEntry<'a>, Resolution<'a>))>,
    /// What the caching fold reads of each minting and composite site the unit typed.
    pub sites: &'a [SiteDeps<'a>],
    /// Each lexical variable a contribution or a solution names, beside the body reading it: what
    /// the contract fold reads.
    pub named_variables: &'a [NamedVariable],
}

/// One body's facts from one unit, each table sorted by its key: the type channel's, then the
/// value channel's, then what an inline body folds into its owner's closing.
#[derive(Clone, Copy, Debug)]
pub struct Typed<'a> {
    /// Each type binder's declaration, by slot.
    pub declared: Keyed<'a, (Slot, StaticType<'a>)>,
    /// Each registration's entry, by slot.
    pub registered: Keyed<'a, (Slot, StaticRegistered<'a>)>,
    /// Each recorded type expression's type, by site.
    pub expressions: Keyed<'a, (Site, StaticType<'a>)>,
    /// Each part's static type, by site.
    pub parts: Keyed<'a, (Site, Interval<Parametric>)>,
    /// Each statement's static type, by index into the body.
    pub statements: Keyed<'a, (u32, Interval<Parametric>)>,
    /// Each binder's static type, by slot.
    pub binders: Keyed<'a, (Slot, DeclaredType<Interval<Parametric>>)>,
    /// Each ascription the walk settled.
    pub settled: Keyed<'a, Site>,
    pub instances: Keyed<'a, (Site, InstanceRead<'a>)>,
    pub uses: Keyed<'a, (Site, UseStatics<'a>)>,
    /// Each selected use's closed points, by group index.
    pub selections: Keyed<'a, (Site, Points<'a>)>,
    /// Each call by name's contributions, by its argument part's site.
    pub by_name: Keyed<'a, (Site, &'a [StaticType<'a>])>,
    /// Each tail call's route, by its node's site.
    pub routes: Keyed<'a, (Site, Route<'a>)>,
    /// Each `:|` at a closed signature, beside its view's content tree.
    pub trees: Keyed<'a, (Site, &'a Tree<'a>)>,
    /// Each `:|` over a body-born module whose content is closed, beside its keyworded members'
    /// picks.
    pub picks: Keyed<'a, (Site, &'a [Pick<'a>])>,
    /// Each module binder's content tree, by slot, where the load read one.
    pub contents: Keyed<'a, (Slot, Tree<'a>)>,
    /// The body's own minting sites.
    pub minting: Keyed<'a, Site>,
    /// A `MODULE` or `GROUP` body's exact signature, where the walk typed one.
    pub signature: Option<KType>,
    /// What the unit's facts give the body's class, folded back in where a resumption absorbs
    /// the unit: a porous exit, how many of its minting sites are views over closed trees, and
    /// the run mints of the blocks and module bodies it ran.
    pub exits: bool,
    pub closed: u32,
    pub folded: u32,
}

impl Typed<'_> {
    /// A body's facts where a unit fixed none.
    pub const EMPTY: Self = Typed {
        declared: Keyed::EMPTY,
        registered: Keyed::EMPTY,
        expressions: Keyed::EMPTY,
        parts: Keyed::EMPTY,
        statements: Keyed::EMPTY,
        binders: Keyed::EMPTY,
        settled: Keyed::EMPTY,
        instances: Keyed::EMPTY,
        uses: Keyed::EMPTY,
        selections: Keyed::EMPTY,
        by_name: Keyed::EMPTY,
        routes: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        picks: Keyed::EMPTY,
        contents: Keyed::EMPTY,
        minting: Keyed::EMPTY,
        signature: None,
        exits: false,
        closed: 0,
        folded: 0,
    };
}

impl UnitFacts<'_> {
    /// A unit's record where it fixed nothing.
    pub const EMPTY: Self = UnitFacts {
        typed: Keyed::EMPTY,
        born: Keyed::EMPTY,
        paths: Keyed::EMPTY,
        sites: &[],
        named_variables: &[],
    };
}

/// A callable or a quote's code, as the unit that births it typed it. Its registration is its
/// callable type's, and the encloser's slot holds its entry.
#[derive(Clone, Copy, Debug)]
pub struct Born<'a> {
    pub callable: StaticCallable<'a>,
    /// Its own `FOR ALL` group as its body reads it, by group order.
    pub levels: &'a [Parametric],
    /// The solution its `FOR ALL` is born instantiated at, where the walk solved one.
    pub instance: Option<StaticInstance<'a>>,
    /// Its parameters whose arguments digest at a constant cost, sorted: a type binder, or one
    /// declared closed under `Module` or as a type value. The keys and caching folds read it.
    pub digestible: &'a [Slot],
}

/// What the walk computes for an owning body once its units and deferred bodies are typed.
#[derive(Clone, Copy, Debug)]
pub struct Closing<'a> {
    pub class: Option<BodyClass>,
    /// A callable's summary: `Some(None)` where it has none, a recursive one among them.
    pub summary: Option<Option<Tree<'a>>>,
    /// Whether the callable lies in a recursive component: it never unfolds.
    pub recursive: bool,
    /// A quote's code's last statement's static type as a traced `EVAL` runs it unfilled.
    pub ran: Option<Interval<Parametric>>,
    /// Why a quote's code does not type, reported when an `EVAL` runs it.
    pub refusal: Option<&'a ShapeError<'a>>,
    /// The first registration of a quote's code that overlaps a builtin overload, reported when an
    /// `EVAL` runs it. Apart from `refusal`, since `USING` keeps code that does not type.
    pub overlap: Option<&'a ShapeError<'a>>,
}

/// What the load folds per body after the walk.
#[derive(Clone, Copy, Debug)]
pub struct Folds<'a> {
    pub caching: Option<&'a Caching<'a>>,
    /// The parameter slots a callable's settlement keys on: `None` where a call seeks nothing.
    pub keys: Option<&'a [Slot]>,
    /// A callable's or a block's parameters, or a quote's code's captures, that some unit beneath
    /// it waits on with its entry as its seam, keyable or not: what its cell's entry binds in its
    /// activation where nothing settled them.
    pub waits: &'a [Address],
    /// A body's slots bound at a statement that some unit's frontier holds: where the cell running
    /// it decides again once the statement binds them.
    pub seams: &'a [Slot],
    /// The seams some path the load left open is rooted at, sorted: decided at their statement
    /// even where the load built their content, which a path reads off what the run binds.
    pub rooting: &'a [Slot],
    /// Which of the body's type captures its content composes, parallel to its layout.
    pub contract: &'a [bool],
}

/// What a site's content depends on: nothing that varies per hop born under one park, the frame
/// body's parameters at these slots, or something that varies regardless.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deps<'a> {
    Closed,
    On(&'a [Slot]),
    Varying,
}

/// The dependencies of content as they gather in scratch while it is walked, laid once at the end:
/// the union of every [`Deps`] added.
pub struct DepsGathering<'x> {
    varying: bool,
    /// Whether the content reads anything open: a capture or a parameter.
    open: bool,
    slots: BumpVec<'x, Slot>,
}

impl<'x> DepsGathering<'x> {
    /// Nothing gathered: closed.
    pub fn new(scratch: BumpAllocator<'x>) -> Self {
        DepsGathering {
            varying: false,
            open: false,
            slots: BumpVec::new_in(scratch),
        }
    }

    /// Gather `deps`.
    pub fn add(&mut self, deps: Deps<'_>) {
        match deps {
            Deps::Varying => self.varying = true,
            Deps::Closed => {}
            Deps::On(slots) => {
                self.open = true;
                self.slots.extend_from_slice(slots);
            }
        }
    }

    /// What is gathered, laid through `lay`: the slots sorted and deduplicated.
    pub fn laid<'a>(&mut self, lay: impl Lays<'a>) -> Deps<'a> {
        if self.varying {
            return Deps::Varying;
        }
        if !self.open {
            return Deps::Closed;
        }
        self.slots.sort_unstable();
        self.slots.dedup();
        Deps::On(collect(lay, self.slots.iter().copied()))
    }
}

/// Gather what the content `tree`, read at `holder` whose frame body is `frame`, depends on: each
/// leaf bound to a parameter of the frame body, no parameter for a leaf captured from a body
/// enclosing the frame, and anything for a leaf no binder names, one bound per run of a body, or
/// one bound by a non-parameter of the frame or of a block or module body it runs.
pub fn tree_deps(
    shapes: &Shapes<'_>,
    (holder, frame): (Segment, Segment),
    tree: &Tree<'_>,
    into: &mut DepsGathering<'_>,
    scratch: BumpAllocator<'_>,
) {
    let mut stack = BumpVec::new_in(scratch);
    stack.push(*tree);
    while let Some(tree) = stack.pop() {
        match tree {
            Tree::Digest(_) => {}
            Tree::Body { captures, .. } => stack.extend(captures.iter().map(|(_, at)| *at)),
            Tree::Knot { members, .. } => {
                for member in members {
                    stack.extend(
                        member
                            .captures
                            .iter()
                            .filter_map(|(_, capture)| match capture {
                                NodeCapture::Tree(tree) => Some(*tree),
                                NodeCapture::Fellow(_) => None,
                            }),
                    );
                }
            }
            Tree::View { source, .. } | Tree::Retyped { source, .. } => stack.push(*source),
            Tree::Leaf(Leaf::Unnamed(_))
            | Tree::Leaf(Leaf::Bound {
                within: Some(_), ..
            }) => return into.add(Deps::Varying),
            Tree::Leaf(Leaf::Bound { binder, .. }) => {
                binder_deps(shapes, (holder, frame), (binder.body, binder.slot), into);
            }
        }
    }
}

/// Gather what the binder at `slot` of `body`, read at `holder` whose frame body is `frame`,
/// depends on: a parameter of the frame body is itself; a capture from a body enclosing the frame
/// is no parameter — open, but the same at every frame of one closure; anything else varies.
fn binder_deps(
    shapes: &Shapes<'_>,
    (holder, frame): (Segment, Segment),
    (body, slot): (Segment, Slot),
    into: &mut DepsGathering<'_>,
) {
    // The chain from the holder up: the bodies the frame runs, the frame, then those around it.
    let mut at = Some(holder);
    let mut past_frame = false;
    while let Some(segment) = at {
        if segment == body {
            let parameter = shapes.shape_at(body).position_of(slot) == Position::PARAMETER;
            let own = [slot];
            return into.add(match (past_frame, segment == frame) {
                (true, _) => Deps::On(&[]),
                (false, true) if parameter => Deps::On(&own),
                _ => Deps::Varying,
            });
        }
        past_frame |= segment == frame;
        at = shapes.parent(segment).map(|(parent, _)| parent);
    }
    into.add(Deps::Varying)
}

/// One minting or composite site the caching fold classes: where it sits, the frame body its
/// dependencies name parameters of, what it depends on, and the path entries it reads, by chain
/// root and level, resolved through the records' resolutions.
#[derive(Clone, Copy, Debug)]
pub struct SiteDeps<'a> {
    pub holder: Segment,
    pub frame: Segment,
    pub site: Site,
    pub minting: bool,
    pub deps: Deps<'a>,
    pub pending: &'a [(Segment, usize)],
}

/// A lexical variable at `level` a contribution or a solution names, read from the body `reading`:
/// every callable or `MODULE` body between composes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NamedVariable {
    pub reading: Segment,
    pub level: usize,
}

/// The fact readers over a source of records: [`LoadFacts`] over one load's, [`Facts`] over a
/// cell's layers and the load they rest on. Every reader takes the body's shape first and finds
/// the fact by its segment, so a reader may hand in a shape borrowed for any lifetime.
#[derive(Clone, Copy)]
pub struct FactsOver<S>(S);

/// Every fact one load decided, by body, beside the shape table they are facts of.
pub type LoadFacts<'graph> = FactsOver<LoadRecords<'graph>>;

/// What a step reads facts through, at the step's own lifetime `'step`: the load its cell's chain
/// rests on, beneath the cell's own layers. Built per step from the cell's arena reference, so
/// keeping one past the step does not compile:
///
/// ```compile_fail
/// use koan::memory::{Writer, resident};
/// use koan::scope::Facts;
///
/// fn keep<'here>(writer: Writer<'here, '_>, facts: Facts<'_>) -> &'here Facts<'here> {
///     resident(writer, facts)
/// }
/// ```
pub type Facts<'step> = FactsOver<Layers<'step>>;

/// One load's records, beside the shape table they are records of.
#[derive(Clone, Copy)]
pub struct LoadRecords<'graph> {
    shapes: &'graph Shapes<'graph>,
    records: &'graph Records<'graph>,
}

/// A cell's layers over the base its chain rests on — the load, or a composition: the events its
/// frame's activation decided, innermost first, then the settlement it runs under, whose parents
/// lie below it. A read tries each layer from the innermost, then the base, and a unit an inner
/// layer re-ran replaces every outer record of it whole.
#[derive(Clone, Copy)]
pub struct Layers<'step> {
    base: LoadRecords<'step>,
    composition: Option<&'step Composition<'step>>,
    top: Option<&'step Settlement<'step>>,
    activation: Option<&'step Activated<'step>>,
}

impl<'step> Facts<'step> {
    /// The facts a cell under the load reads: the load's alone.
    pub fn of(load: &LoadFacts<'step>) -> Self {
        Facts::under(load, None)
    }

    /// The facts a cell under the settlement `top` reads: its layers over the load.
    pub fn under(load: &LoadFacts<'step>, top: Option<&'step Settlement<'step>>) -> Self {
        FactsOver(Layers {
            base: load.0,
            composition: None,
            top,
            activation: None,
        })
    }

    /// The facts a cell whose chain rests on `composition` reads: its layers over the
    /// composition's, under the settlement `top`.
    pub fn composed(
        composition: &'step Composition<'step>,
        top: Option<&'step Settlement<'step>>,
    ) -> Self {
        FactsOver(Layers {
            base: composition.facts().0,
            composition: Some(composition),
            top,
            activation: None,
        })
    }

    /// The composition the cell's chain rests on, if not the load.
    pub fn composition(&self) -> Option<&'step Composition<'step>> {
        self.0.composition
    }

    /// These facts beneath `activation`, the events a frame's activation decided, in place of any
    /// these read.
    pub fn activated(self, activation: Option<&'step Activated<'step>>) -> Self {
        FactsOver(Layers {
            activation,
            ..self.0
        })
    }

    /// The innermost settlement these facts read, if any.
    pub fn top(&self) -> Option<&'step Settlement<'step>> {
        self.0.top
    }

    /// `fact`, read off these facts, with every type it names resolving in `types`, the frame's
    /// chain: each the chain lacks is minted from its term, found by digest in the composition's
    /// table and then each settlement's. A frame mints only what it reads, once per tier the chain
    /// interns in. Over the load and the settlements under the root, every type a fact names lies
    /// in the root or the agent tier already.
    pub fn minted<T: Named>(&self, types: &Chain<'_, '_, '_, '_>, fact: T) -> T {
        let Some(composition) = self.0.composition else {
            return fact;
        };
        fact.each_named(&mut |kt| {
            if types.resolves(kt) {
                return;
            }
            #[cfg(test)]
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            let digest = kt.digest();
            let term = composition.terms().get(digest).or_else(|| {
                std::iter::successors(self.0.top, |layer| layer.parent())
                    .find_map(|layer| layer.terms().get(digest))
            });
            match term {
                Some((_, term)) => types.mint(term),
                None => debug_assert!(false, "a type a fact names has a term"),
            }
        });
        fact
    }

    /// Hand `each` every term a frame reading these facts mints so every type they name resolves
    /// in its chain, in the order a term's leaves resolve: the composition's, then each
    /// settlement's from the outermost. None over the load.
    pub fn each_term(&self, each: &mut dyn FnMut(&'step Term<'step>)) {
        #[cfg(test)]
        let each = &mut |term| {
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            each(term)
        };
        let terms = |composition: &'step Composition<'step>| composition.terms().entries();
        (self.0.composition.map_or(&[][..], terms).iter()).for_each(|(_, term)| each(term));
        let mut settlements: smallvec::SmallVec<[&Settlement<'step>; 4]> =
            std::iter::successors(self.0.top, |layer| layer.parent()).collect();
        settlements.reverse();
        (settlements.iter())
            .flat_map(|layer| layer.terms().entries())
            .for_each(|(_, term)| each(term));
    }

    /// `fact`, a fact of `shape` read at `site` where the cell runs: closed, refused at every run,
    /// or left open to the run's own door. A rigid fact never reaches the run — every variable the
    /// load left open is bound in a layer the cell reads — save one over a path the load read off a
    /// signature lacking its member, which every run refuses; any other is a bug, refused as the
    /// load refuses a name with no binding.
    pub fn at_run<C, R>(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        fact: Static<'step, C, R>,
    ) -> AtRun<C> {
        match fact {
            Static::Closed(value) => AtRun::Closed(value),
            Static::Unknown => AtRun::Open,
            Static::Rigid { variables, .. } => AtRun::Refused(self.unbound(shape, site, variables)),
        }
    }

    /// The type the type expression at `site` of `shape` denotes where the cell runs, or why every
    /// run refuses it. The load, or a layer the cell reads, fixed it: one left open is a bug.
    pub fn denoted(&self, shape: &BodyShape<'_>, site: Site) -> Result<KType, Elaboration> {
        match self.at_run(shape, site, self.typed_expression(shape, site)) {
            AtRun::Closed(closed) => Ok(closed),
            AtRun::Refused(refused) => Err(refused),
            AtRun::Open => {
                debug_assert!(
                    false,
                    "the type at {site:?} of body {:?} is open where it runs",
                    shape.segment()
                );
                Err(Elaboration::Unknown { site })
            }
        }
    }

    /// Why a rigid fact of `shape` over `variables`, read at `site`, refuses.
    fn unbound(&self, shape: &BodyShape<'_>, site: Site, variables: &[Variable]) -> Elaboration {
        let shapes = self.shapes();
        let path = |root: Segment, level| self.path(shapes.shape_at(root), level);
        for variable in variables {
            if let Source::Path { root, level, .. } = variable.source
                && let Some((entry, resolution)) = path(root, level)
                && let Some(owner) = resolution.lacks
            {
                return Elaboration::NoSuchMember {
                    owner: owner.into(),
                    name: entry.label.symbol(),
                    site,
                };
            }
        }
        #[cfg(test)]
        rigid_at_run();
        debug_assert!(
            false,
            "a rigid fact of body {:?} at {site:?} reached the run",
            shape.segment()
        );
        let name = variables.iter().find_map(|variable| match variable.source {
            Source::Type(at) => match shapes.follow(shape.segment(), at)? {
                Lands::Binder { at, slot } => Some(shapes.shape_at(at).slot_name(slot).symbol()),
                _ => None,
            },
            Source::Path { root, level, .. } => Some(path(root, level)?.0.label.symbol()),
        });
        match name {
            Some(name) => Elaboration::Unbound { name, site },
            None => Elaboration::Unknown { site },
        }
    }

    /// The wording of the refusal the innermost settlement's walk reached, else the refusal of the
    /// composed code the chain rests on, if either reached one.
    pub fn refusal(&self) -> Option<&'step str> {
        match self.0.top {
            Some(top) => top.refusal(),
            None => self.0.composition?.refusal(),
        }
    }
}

impl<'step> Layers<'step> {
    /// The activation's events, innermost first.
    fn decided(&self) -> impl Iterator<Item = &'step Activated<'step>> {
        std::iter::successors(self.activation, |layer| layer.below())
    }

    /// Each layer's records, innermost first: the activation's, then the settlements'.
    fn layers(&self) -> impl Iterator<Item = &'step Records<'step>> {
        (self.decided().map(|layer| layer.records())).chain(
            std::iter::successors(self.top, |layer| layer.parent()).map(|layer| layer.records()),
        )
    }

    /// Whether a layer inner to `outer` — every layer where `outer` is `None` — re-ran `unit`.
    fn rerun_within(&self, outer: Option<&Records<'_>>, unit: UnitId) -> bool {
        (self.layers())
            .take_while(|layer| outer.is_none_or(|outer| !std::ptr::addr_eq(*layer, outer)))
            .any(|layer| layer.units.get(unit).is_some())
    }
}

/// Where the fact readers find a record. A load's own records lie over its base's, and a re-run
/// unit's record replaces the base's whole.
pub(crate) trait RecordSource<'a>: Copy {
    fn shapes(&self) -> &'a Shapes<'a>;

    /// What the newest walk over these records bound: each layer's binding extends the one below.
    /// A frame's activation binds more, which [`bound`](Self::bound) reads.
    fn binding(&self) -> Binding<'a>;

    /// What `unknown` is bound to, by any layer.
    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.binding().bound(unknown)
    }

    /// Hand `each` every unknown any layer binds, beside what it binds it to.
    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        for (unknown, bound) in self.binding().entries() {
            each(*unknown, *bound);
        }
    }

    /// The newest record of `unit`, beside its frontier.
    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)>;

    /// The newest closing record of the owning body `body`, beside its frontier.
    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)>;

    /// The record holding `held`, beside its unit.
    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)>;

    /// Hand `each` every path entry of the chain `root` roots that any record holds, an entry
    /// perhaps more than once.
    fn each_path(&self, root: Segment, each: &mut dyn FnMut(PathEntry<'a>));

    /// `body` as its load laid it.
    fn laid(&self, body: Segment) -> &'a Laid<'a>;

    /// The folds of `body`.
    fn folds(&self, body: Segment) -> Option<Folds<'a>>;

    /// The closed tree whose identity is `digest`.
    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>>;
}

impl<'graph> RecordSource<'graph> for LoadRecords<'graph> {
    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    fn binding(&self) -> Binding<'graph> {
        self.records.binding
    }

    fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        self.records.unit(unit)
    }

    fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        self.records.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'graph UnitFacts<'graph>)> {
        self.records.holding(held)
    }

    fn each_path(&self, root: Segment, each: &mut dyn FnMut(PathEntry<'graph>)) {
        self.records.each_path(root, each);
    }

    fn laid(&self, body: Segment) -> &'graph Laid<'graph> {
        self.records.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'graph>> {
        self.records.folds(body)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'graph Tree<'graph>> {
        match self.records.trees.get(digest) {
            Some((_, tree)) => Some(tree),
            None => self.records.base?.0.tree_of(digest),
        }
    }
}

impl<'step> RecordSource<'step> for Layers<'step> {
    fn shapes(&self) -> &'step Shapes<'step> {
        self.base.shapes()
    }

    fn binding(&self) -> Binding<'step> {
        match self.top {
            Some(top) => top.records().binding,
            None => self.base.binding(),
        }
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'step>> {
        (self.decided())
            .find_map(|layer| layer.records().binding.bound(unknown))
            .or_else(|| self.binding().bound(unknown))
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'step>)) {
        let own = self.decided().map(|layer| layer.records().binding);
        for binding in own.chain([self.binding()]) {
            for (unknown, bound) in binding.entries() {
                each(*unknown, *bound);
            }
        }
    }

    fn unit(&self, unit: UnitId) -> Option<(&'step UnitFacts<'step>, Frontier<'step>)> {
        (self.layers())
            .find_map(|layer| layer.units.get(unit).map(|(_, record)| *record))
            .or_else(|| self.base.unit(unit))
    }

    fn closing(&self, body: Segment) -> Option<(&'step Closing<'step>, Frontier<'step>)> {
        (self.layers())
            .find_map(|layer| layer.closings.get(body).map(|(_, record)| *record))
            .or_else(|| self.base.closing(body))
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'step UnitFacts<'step>)> {
        for layer in self.layers() {
            if let Some((_, at)) = layer.index.get(held) {
                let (unit, (facts, _)) = layer.units[*at as usize];
                return (!self.rerun_within(Some(layer), unit)).then_some((unit, facts));
            }
        }
        let (unit, facts) = self.base.holding(held)?;
        (!self.rerun_within(None, unit)).then_some((unit, facts))
    }

    fn each_path(&self, root: Segment, each: &mut dyn FnMut(PathEntry<'step>)) {
        for layer in self.layers() {
            layer.each_path(root, each);
        }
        self.base.each_path(root, each);
    }

    fn laid(&self, body: Segment) -> &'step Laid<'step> {
        self.base.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'step>> {
        self.base.folds(body)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'step Tree<'step>> {
        (self.layers())
            .find_map(|layer| layer.trees.get(digest).map(|(_, tree)| *tree))
            .or_else(|| self.base.tree_of(digest))
    }
}

// A layer may rest on any fact reader: a resumption's base is the facts it extends.
impl<'a, S: RecordSource<'a>> RecordSource<'a> for FactsOver<S> {
    fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    fn binding(&self) -> Binding<'a> {
        self.0.binding()
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.0.bound(unknown)
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        self.0.each_bound(each);
    }

    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        self.0.unit(unit)
    }

    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)> {
        self.0.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)> {
        self.0.holding(held)
    }

    fn each_path(&self, root: Segment, each: &mut dyn FnMut(PathEntry<'a>)) {
        self.0.each_path(root, each);
    }

    fn laid(&self, body: Segment) -> &'a Laid<'a> {
        self.0.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'a>> {
        self.0.folds(body)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }
}

/// The code a value is born of, read at the program's own lifetime: the program's written code,
/// or, for a cell whose chain rests on a composition, the code composed at run time, under its root
/// code value. What a value born of it keeps is its body's record or a quote's syntax. The load is
/// never evicted, so a value may point into it; a composition may be, so a value born of composed
/// code keeps its own copy of its body's record and the root that finds or rebuilds the
/// composition, and its quote's syntax inside the root's. Every fact a step decides by is read
/// through [`Facts`] instead.
#[derive(Clone, Copy)]
pub struct Written<'graph, 'step> {
    load: LoadRecords<'graph>,
    composed: Option<(ComposedRoot<'graph>, &'step Composition<'step>)>,
}

/// A body's record as a value born of it holds it: the load's, by pointer, or one of composed code,
/// its root included, copied into the value's region.
#[derive(Clone, Copy)]
pub enum BodyAt<'graph, 'cell> {
    Written(&'graph BodyRef<'graph, 'graph>),
    Composed(&'cell BodyRef<'graph, 'cell>),
}

impl<'graph: 'cell, 'cell> BodyAt<'graph, 'cell> {
    /// The record itself.
    pub fn get(self) -> &'cell BodyRef<'graph, 'cell> {
        match self {
            BodyAt::Written(body) => body,
            BodyAt::Composed(body) => body,
        }
    }

    /// This record as a value copied through `lay` holds it: the load's pointer kept, composed
    /// code's record copied.
    pub fn copied<'to>(self, lay: impl Lays<'to>) -> BodyAt<'graph, 'to>
    where
        'graph: 'to,
    {
        match self {
            BodyAt::Written(body) => BodyAt::Written(body),
            BodyAt::Composed(body) => BodyAt::Composed(body.laid(lay, body.root)),
        }
    }
}

impl<'graph, 'step> Written<'graph, 'step> {
    /// The written code `load` holds.
    pub fn of(load: &LoadFacts<'graph>) -> Self {
        Written {
            load: load.0,
            composed: None,
        }
    }

    /// The code composed at run time whose root is `root`, which `composition` holds.
    pub fn composed(
        self,
        root: ComposedRoot<'graph>,
        composition: &'step Composition<'step>,
    ) -> Self {
        Written {
            composed: Some((root, composition)),
            ..self
        }
    }

    /// The root of the composed code this is, if it is not the program's.
    pub fn root(&self) -> Option<ComposedRoot<'graph>> {
        self.composed.map(|(root, _)| root)
    }

    /// The body at `segment`, as a value born of it holds it, a composed record laid through `lay`
    /// beside its root.
    pub fn body<'cell>(&self, lay: impl Lays<'cell>, segment: Segment) -> BodyAt<'graph, 'cell>
    where
        'graph: 'cell,
    {
        match self.composed {
            None => BodyAt::Written(&self.load.laid(segment).body),
            Some((root, composition)) => BodyAt::Composed(
                composition
                    .facts()
                    .0
                    .laid(segment)
                    .body
                    .laid(lay, Some(root)),
            ),
        }
    }

    /// The syntax of the quote at `site` of the body at `segment`.
    pub fn quote(&self, segment: Segment, site: Site) -> ProgramNode<'graph> {
        let Some((root, composition)) = self.composed else {
            return match self.load.shapes.shape_at(segment).located(site) {
                Located::Part(ExpressionPart::QuotedExpression(node)) => *node,
                _ => unreachable!("a quote's site locates its quoted expression"),
            };
        };
        let shapes = composition.facts().shapes();
        let nested = (shapes.shape_at(segment).nested(site))
            .expect("a quote's site holds its code shape")
            .segment();
        composition.quote_in(root.syntax, nested)
    }
}

/// A body as a value born of it holds it: the base whose shape table holds it, by digest, its
/// segment there, its code digest, and which of its closure slots its content composes — its
/// builder captures but those reading the program's top level, then the type captures its contract
/// names. A value's identity, digest and equality read those alone. Beside them, what a caller
/// needs of a body it did not shape: the slots a call's settlement keys on, by name, and the root
/// of the composed code it lies in, which finds or rebuilds its composition. One pointer, so a
/// value holds it in place of its shape's.
#[derive(Clone, Copy)]
pub struct BodyRef<'graph, 'a> {
    pub base: ContentDigest,
    pub segment: Segment,
    pub code: ContentDigest,
    pub composes: &'a [bool],
    /// The parameter slots a call's settlement keys on, beside their names: `None` where a call
    /// seeks nothing.
    pub keys: Option<&'a [(Slot, BinderSymbol)]>,
    /// `None` for written code.
    pub root: Option<ComposedRoot<'graph>>,
}

/// The root of code composed at run time: its syntax, which lives in program storage until code
/// splicing lays composed syntax in regions, and its syntax digest, which finds its composition.
#[derive(Clone, Copy, Debug)]
pub struct ComposedRoot<'a> {
    pub syntax: ProgramNode<'a>,
    pub digest: ContentDigest,
}

impl<'graph> BodyRef<'graph, '_> {
    /// Whether the closure slot `at` composes: one past the slots recorded does.
    pub fn composed(&self, at: usize) -> bool {
        self.composes.get(at).copied().unwrap_or(true)
    }

    /// This record copied through `lay` beside `root`.
    pub fn laid<'root, 'to>(
        &self,
        lay: impl Lays<'to>,
        root: Option<ComposedRoot<'root>>,
    ) -> &'to BodyRef<'root, 'to> {
        resident(
            lay,
            BodyRef {
                base: self.base,
                segment: self.segment,
                code: self.code,
                composes: collect(lay, self.composes.iter().copied()),
                keys: self.keys.map(|keys| collect(lay, keys.iter().copied())),
                root,
            },
        )
    }

    /// What laying a copy of this record costs a rebuild: nothing for written code, whose record
    /// the load holds.
    pub fn weight(&self) -> Weight {
        match self.root {
            None => Weight::ZERO,
            Some(_) => Weight::flat::<BodyRef<'_, '_>>()
                .plus(Weight::run::<bool>(self.composes.len()))
                .plus(Weight::run::<(Slot, BinderSymbol)>(
                    self.keys.map_or(0, <[_]>::len),
                )),
        }
    }
}

/// Which closure slots of `shape` its content composes: each builder capture reading no top-level
/// slot, then each type capture as `contract` says.
fn composes<'graph>(
    lay: impl Lays<'graph>,
    shape: &BodyShape<'_>,
    contract: impl Fn(usize) -> bool,
    types: usize,
) -> &'graph [bool] {
    let builder = shape.captures().len();
    lay.fill(builder + types, |at| match at.checked_sub(builder) {
        None => shape.composes(super::shape::CaptureSlot(at as u32)),
        Some(index) => contract(index),
    })
}

/// One walk's records, over its base's where it has one.
#[derive(Clone, Copy)]
pub(super) struct Records<'graph> {
    base: Option<&'graph LoadFacts<'graph>>,
    /// What the walk bound: the base's binding and more.
    binding: Binding<'graph>,
    /// Each unit the walk ran, beside its record and the frontier it gathered.
    units: Keyed<'graph, (UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))>,
    /// Each owning body the walk closed.
    closings: Keyed<'graph, (Segment, (&'graph Closing<'graph>, Frontier<'graph>))>,
    /// Which of `units` holds each site, through and path fact: the facts the shape cannot place.
    index: Keyed<'graph, (Held, u32)>,
    /// Each closed tree `units` computed, a subtree included, by its identity.
    trees: Keyed<'graph, (ContentDigest, &'graph Tree<'graph>)>,
    /// Each body, by segment, beside its type captures as its birth reads them and its folds: none
    /// before the load folds, when a read falls to the base.
    bodies: &'graph [Laid<'graph>],
}

/// One body as a [`Records`] load lays it.
#[derive(Clone, Copy)]
pub(crate) struct Laid<'graph> {
    body: BodyRef<'graph, 'graph>,
    captures: &'graph [Coordinate],
    folds: Option<Folds<'graph>>,
}

/// A fact a unit's record holds that the shape table cannot place, by what it is found by. A slot's,
/// a statement's and a birth's unit is the shape's to name: [`Shapes::slot_unit`],
/// [`Shapes::running`] and [`Shapes::birth_unit`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Held {
    Site(Segment, Site),
    /// A path entry, by its chain root and level.
    Path(Segment, usize),
}

impl<'graph> LoadFacts<'graph> {
    /// The facts of a load that built no shape: what a graph that runs no program reads.
    pub const EMPTY: Self = FactsOver(LoadRecords {
        shapes: &Shapes::EMPTY,
        records: &Records {
            base: None,
            binding: Binding::EMPTY,
            units: Keyed::EMPTY,
            closings: Keyed::EMPTY,
            index: Keyed::EMPTY,
            trees: Keyed::EMPTY,
            bodies: &[],
        },
    });

    /// The bytes of this load's records index.
    #[cfg(test)]
    pub fn index_bytes(&self) -> usize {
        std::mem::size_of_val(self.0.records.index.entries())
    }

    /// The bytes of the facts this load's records index keys: each own record's paths, and each
    /// body's site-keyed and through tables in it.
    #[cfg(test)]
    pub fn keyed_fact_bytes(&self) -> usize {
        use std::mem::size_of_val;
        let typed = |typed: &Typed<'_>| {
            size_of_val(typed.expressions.entries())
                + size_of_val(typed.parts.entries())
                + size_of_val(typed.settled.entries())
                + size_of_val(typed.instances.entries())
                + size_of_val(typed.uses.entries())
                + size_of_val(typed.selections.entries())
                + size_of_val(typed.by_name.entries())
                + size_of_val(typed.routes.entries())
                + size_of_val(typed.trees.entries())
                + size_of_val(typed.picks.entries())
                + size_of_val(typed.minting.entries())
        };
        (self.own_units().iter())
            .map(|(_, (facts, _))| {
                size_of_val(facts.paths.entries())
                    + facts.typed.iter().map(|(_, at)| typed(at)).sum::<usize>()
            })
            .sum()
    }

    /// The facts of a load whose walk ran nothing over `shapes`: every fact unknown, and every
    /// capture composing.
    #[cfg(test)]
    pub fn unwalked(lay: impl Lays<'graph>, shapes: &'graph Shapes<'graph>) -> Self {
        let records = Records {
            base: None,
            binding: Binding::EMPTY,
            units: Keyed::from_sorted(&[]),
            closings: Keyed::from_sorted(&[]),
            index: Keyed::from_sorted(&[]),
            trees: Keyed::EMPTY,
            bodies: &[],
        };
        FactsOver(LoadRecords {
            shapes,
            records: resident(lay, records),
        })
        .folded(lay, &[])
    }

    /// One walk's records over `shapes`, laid through `lay`, over `base` where the walk resumed
    /// one under `binding`: `units` and `closings` sorted by id, not yet folded.
    pub fn of_records(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        shapes: &'graph Shapes<'graph>,
        base: Option<&'graph LoadFacts<'graph>>,
        binding: Binding<'graph>,
        units: &[(UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))],
        closings: &[(Segment, (&'graph Closing<'graph>, Frontier<'graph>))],
    ) -> Self {
        debug_assert!(base.is_none_or(|base| binding.extends(base.binding())));
        debug_assert!(base.is_none_or(|base| std::ptr::eq(base.0.shapes, shapes)));
        let records = Records::of(lay, scratch, shapes, base, binding, units, closings);
        FactsOver(LoadRecords {
            shapes,
            records: resident(lay, records),
        })
    }

    /// These records beside `folds`, by segment: every capture composing where it is empty.
    pub fn folded(&self, lay: impl Lays<'graph>, folds: &[Folds<'graph>]) -> Self {
        let records = self.0.records;
        let shapes = self.0.shapes;
        debug_assert!(folds.is_empty() || folds.len() == shapes.bodies().len());
        let bodies = shapes.bodies();
        let base = shapes.root().code_digest();
        let bodies = lay.fill(bodies.len(), |at| {
            let shape = bodies[at];
            let layout = shapes.layout(shape.segment());
            let folds = folds.get(at).copied();
            let contract = |index: usize| folds.is_none_or(|folds| folds.contract[index]);
            Laid {
                body: BodyRef {
                    base,
                    segment: shape.segment(),
                    code: shape.code_digest(),
                    composes: composes(lay, shape, contract, layout.len()),
                    keys: folds.and_then(|folds| folds.keys).map(|keys| {
                        collect(lay, keys.iter().map(|slot| (*slot, shape.slot_name(*slot))))
                    }),
                    root: None,
                },
                captures: collect(lay, layout.iter().map(|(_, coordinate)| *coordinate)),
                folds,
            }
        });
        FactsOver(LoadRecords {
            shapes,
            records: resident(lay, Records { bodies, ..*records }),
        })
    }

    /// The newest record of `unit` beside its frontier, its base's where this load did not re-run
    /// it.
    pub fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        self.0.records.unit(unit)
    }

    /// The newest closing record of the owning body `body`, beside its frontier.
    pub fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        self.0.records.closing(body)
    }

    /// The units this load ran itself, each beside its record and frontier, sorted.
    /// Hand `each` every type these records name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        self.0.records.each_type(each);
    }

    pub fn own_units(&self) -> &'graph [(UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))] {
        self.0.records.units.entries()
    }
}

#[expect(
    private_bounds,
    reason = "a source of records is the crate's own; outside it only the two aliases are named"
)]
impl<'a, S: RecordSource<'a>> FactsOver<S> {
    /// The shape table these are facts of.
    pub fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    /// What the newest walk over these records bound: nothing, at the load.
    pub fn binding(&self) -> Binding<'a> {
        self.0.binding()
    }

    /// The closed tree whose identity is `digest`, among those these records computed.
    pub fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }

    /// `segment`'s facts in the record holding `held`.
    fn typed(&self, segment: Segment, held: Held) -> Option<&'a Typed<'a>> {
        let (_, facts) = self.0.holding(held)?;
        facts.typed.get(segment).map(|(_, typed)| typed)
    }

    /// `shape` as this load's table holds it, found by its segment: so a reader's shape may be
    /// borrowed for any lifetime.
    fn at(&self, shape: &BodyShape<'_>) -> &'a BodyShape<'a> {
        let at = self.0.shapes().shape_at(shape.segment());
        debug_assert!(std::ptr::addr_eq(at, shape), "a shape of this load");
        at
    }

    /// The entry at `key` of the `table` of the facts the unit holding `held` fixed for `shape`.
    fn fact<K: Ord + Copy + 'a, V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        held: Held,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (K, V)>,
        key: K,
    ) -> Option<V> {
        let typed = self.typed(shape.segment(), held)?;
        table(typed).get(key).map(|(_, value)| *value)
    }

    /// The entry at `site` of the site-keyed `table` of `shape`'s facts.
    fn at_site<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Site, V)>,
    ) -> Option<V> {
        self.fact(shape, Held::Site(shape.segment(), site), table, site)
    }

    /// `shape`'s facts in the newest record of `unit`.
    fn typed_in(&self, shape: &BodyShape<'_>, unit: UnitId) -> Option<&'a Typed<'a>> {
        let (facts, _) = self.0.unit(unit)?;
        facts.typed.get(shape.segment()).map(|(_, typed)| typed)
    }

    /// The entry at `slot` of the slot-keyed `table` of `shape`'s facts.
    fn at_slot<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        slot: Slot,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Slot, V)>,
    ) -> Option<V> {
        let typed = self.typed_in(shape, self.0.shapes().slot_unit(shape.segment(), slot))?;
        table(typed).get(slot).map(|(_, value)| *value)
    }

    /// The birth facts of `body`, a callable or a quote's code, in its birth unit's newest record.
    fn born(&self, body: Segment) -> Option<&'a Born<'a>> {
        let (facts, _) = self.0.unit(self.0.shapes().birth_unit(body)?)?;
        facts.born.get(body).map(|(_, born)| born)
    }

    /// Whether the site-listing `table` of `shape`'s facts lists `site`.
    fn lists_site(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, Site>,
    ) -> bool {
        (self.typed(shape.segment(), Held::Site(shape.segment(), site)))
            .is_some_and(|typed| table(typed).get(site).is_some())
    }

    /// What the load fixed for the type expression at `site` of `shape`: `Unknown` where it records
    /// none there.
    pub fn typed_expression(&self, shape: &BodyShape<'_>, site: Site) -> StaticType<'a> {
        (self.at_site(shape, site, |typed| typed.expressions)).unwrap_or(Static::Unknown)
    }

    /// What the load fixed for the type binder at `slot` of `shape`.
    pub fn declared_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticType<'a> {
        (self.at_slot(shape, slot, |typed| typed.declared)).unwrap_or(Static::Unknown)
    }

    /// What the load fixed for the registration at `slot` of `shape`.
    pub fn registered_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticRegistered<'a> {
        (self.at_slot(shape, slot, |typed| typed.registered)).unwrap_or(Static::Unknown)
    }

    /// What the load fixed for the callable body `shape`'s type.
    pub fn callable_type(&self, shape: &BodyShape<'_>) -> StaticCallable<'a> {
        (self.born(shape.segment())).map_or(Static::Unknown, |born| born.callable)
    }

    /// The callable body `shape`'s own `FOR ALL` group as its body reads it, by group order.
    pub fn group_levels(&self, shape: &BodyShape<'_>) -> &'a [Parametric] {
        self.born(shape.segment()).map_or(&[], |born| born.levels)
    }

    /// The solution the callable body `shape`'s `FOR ALL` is born instantiated at, where the load
    /// solved one.
    pub fn born_instance(&self, shape: &BodyShape<'_>) -> Option<StaticInstance<'a>> {
        self.born(shape.segment())?.instance
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub fn homes(&self, shape: &BodyShape<'_>) -> &'a [(usize, Target)] {
        self.0.shapes().homes(shape.segment())
    }

    /// Each type capture of `shape` past its builder captures, as the coordinate its birth reads.
    pub fn type_captures(&self, shape: &BodyShape<'_>) -> &'a [Coordinate] {
        self.0.laid(shape.segment()).captures
    }

    /// How many closure slots a birth of `shape` fills: its builder captures, then its type
    /// captures.
    pub fn capture_count(&self, shape: &BodyShape<'_>) -> usize {
        shape.captures().len() + self.type_captures(shape).len()
    }

    /// Which of `shape`'s type captures its content composes, parallel to
    /// [`type_captures`](Self::type_captures).
    pub fn contract(&self, shape: &BodyShape<'_>) -> &'a [bool] {
        &self.0.laid(shape.segment()).body.composes[shape.captures().len()..]
    }

    /// The path `shape` spells at `site`, where it spells one its chain numbers: the segment of
    /// the body rooting its chain, beside its level there. A text fact, read off the shape table.
    pub fn path_read(&self, shape: &BodyShape<'_>, site: Site) -> Option<(Segment, usize)> {
        let index = shape.spelled_at(site)?;
        let level = self.0.shapes().path_level(shape.segment(), index)?;
        Some((self.0.shapes().chain_root(shape.segment()), level))
    }

    /// The path entry at `level` of the chain the body `root` roots, beside what the load resolved
    /// it to.
    pub fn path(
        &self,
        root: &BodyShape<'_>,
        level: usize,
    ) -> Option<(PathEntry<'a>, Resolution<'a>)> {
        let (_, facts) = self.0.holding(Held::Path(root.segment(), level))?;
        facts
            .paths
            .get((root.segment(), level))
            .map(|(_, path)| *path)
    }

    /// The first registration of the quote's code `shape` that overlaps a builtin overload,
    /// reported when an `EVAL` runs it.
    pub fn overlap(&self, shape: &BodyShape<'_>) -> Option<&'a ShapeError<'a>> {
        (self.0.closing(shape.segment())).and_then(|(closing, _)| closing.overlap)
    }

    /// Why the code shape `shape`'s code cannot be built or does not type, reported when an `EVAL`
    /// runs it.
    pub fn typing_refusal(&self, shape: &BodyShape<'_>) -> Option<&'a ShapeError<'a>> {
        // A code shape the builder could not build holds its refusal itself.
        (self.0.closing(shape.segment()))
            .and_then(|(closing, _)| closing.refusal)
            .or_else(|| self.at(shape).refusal())
    }

    /// The static type of the part at `site` of `shape`, where the load typed one.
    pub fn value_type(&self, shape: &BodyShape<'_>, site: Site) -> Option<Interval> {
        self.at_site(shape, site, |typed| typed.parts)
    }

    /// The static type of statement `index` of `shape`'s body.
    pub fn statement_type(&self, shape: &BodyShape<'_>, index: usize) -> Option<Interval> {
        let index = index as u32;
        let typed = self.typed_in(shape, self.0.shapes().running(shape.segment(), index))?;
        typed.statements.get(index).map(|(_, typed)| *typed)
    }

    /// The static type of the binder at `slot` of `shape`.
    pub fn binder_type(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<DeclaredType<Interval>> {
        self.at_slot(shape, slot, |typed| typed.binders)
    }

    /// What the load fixed about the keyworded use whose node sits at `site` of `shape`.
    pub fn use_statics(&self, shape: &BodyShape<'_>, site: Site) -> UseStatics<'a> {
        (self.at_site(shape, site, |typed| typed.uses)).unwrap_or(UseStatics::UNFIXED)
    }

    /// The route the load fixed for the tail call whose node sits at `site` of `shape`, where it
    /// knows the callee.
    pub fn route(&self, shape: &BodyShape<'_>, site: Site) -> Option<Route<'a>> {
        self.at_site(shape, site, |typed| typed.routes)
    }

    /// The class the load fixed for the frame body `shape`.
    pub fn class(&self, shape: &BodyShape<'_>) -> Option<BodyClass> {
        self.0.closing(shape.segment())?.0.class
    }

    /// Whether the frame body `shape` caches.
    pub fn caches(&self, shape: &BodyShape<'_>) -> bool {
        self.caching(shape).is_some_and(|caching| caching.caches)
    }

    /// Where `shape`'s types intern at run.
    pub fn caching(&self, shape: &BodyShape<'_>) -> Option<&'a Caching<'a>> {
        self.0.folds(shape.segment())?.caching
    }

    /// How the load classed the minting site at `site`, one of `shape`'s own.
    pub fn mint_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<Mint> {
        let caching = self.caching(shape)?;
        caching.mints.get(site).map(|(_, mint)| *mint)
    }

    /// Whether the load recorded `site` as one of `shape`'s own minting sites.
    pub fn mints_at(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        self.lists_site(shape, site, |typed| typed.minting)
    }

    /// The content tree of the view the `:|` at `site` of `shape` builds, where its signature is
    /// closed.
    pub fn view_tree(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a Tree<'a>> {
        self.at_site(shape, site, |typed| typed.trees)
    }

    /// What the view the `:|` at `site` of `shape` builds carries for each keyworded member, where
    /// the load sees all of the body-born module it views.
    pub fn picks(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a [Pick<'a>]> {
        self.at_site(shape, site, |typed| typed.picks)
    }

    /// Whether the load settled the ascription at `site` of `shape`.
    pub fn settled(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        self.lists_site(shape, site, |typed| typed.settled)
    }

    /// What the load fixed where `shape` reads a quantified function at `site`.
    pub fn instance_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<InstanceRead<'a>> {
        self.at_site(shape, site, |typed| typed.instances)
    }

    /// The point each variable of the group of the candidate the keyworded use at `site` of
    /// `shape` selects solves to, by group index, where the load selected one.
    pub fn selection(&self, shape: &BodyShape<'_>, site: Site) -> Option<Points<'a>> {
        self.at_site(shape, site, |typed| typed.selections)
    }

    /// The content tree of the module the binder at `slot` of `shape` holds, if the load read it.
    pub fn content(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<Tree<'a>> {
        self.at_slot(shape, slot, |typed| typed.contents)
    }

    /// What each parameter of the call by name whose argument sits at `site` of `shape` is solved
    /// from.
    pub fn named_contributions(&self, shape: &BodyShape<'_>, site: Site) -> &'a [StaticType<'a>] {
        (self.at_site(shape, site, |typed| typed.by_name)).unwrap_or(&[])
    }

    /// The slots `shape` binds at a statement that some unit's frontier holds.
    pub fn rooting_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.folds(shape.segment())
            .map_or(&[], |folds| folds.rooting)
    }

    pub fn statement_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.seams)
    }

    /// The parameters of the callable or block `shape`, or the captures of the quote's code
    /// `shape`, some unit beneath it waits on with its entry as its seam.
    pub fn entry_waits(&self, shape: &BodyShape<'_>) -> &'a [Address] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.waits)
    }

    /// How many of `shape`'s slots keep a digest memo.
    pub fn digested_count(&self, shape: &BodyShape<'_>) -> usize {
        self.0.shapes().digested_count(shape.segment())
    }

    /// Where `slot`'s digest memo sits among `shape`'s, if it keeps one.
    pub fn digested_index(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<usize> {
        self.0.shapes().digested_index(shape.segment(), slot)
    }

    /// Where the captures of the module the `:|` at `site` of `shape` views keep their memos.
    pub fn view_slots(&self, shape: &BodyShape<'_>, site: Site) -> Option<ViewSlots<'a>> {
        self.0.shapes().view_slots(shape.segment(), site)
    }

    /// The digested slots the call keyed by `site` of `shape` passes bare.
    pub fn handoffs(&self, shape: &BodyShape<'_>, site: Site) -> &'a [Handoff] {
        self.0.shapes().handoffs(shape.segment(), site)
    }
}

impl<'graph> Records<'graph> {
    /// One walk's records over `shapes`, laid through `lay`, over `base` where it has one, under
    /// `binding`: `units` and `closings` sorted by id, not yet folded.
    pub(super) fn of(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        shapes: &Shapes<'_>,
        base: Option<&'graph LoadFacts<'graph>>,
        binding: Binding<'graph>,
        units: &[(UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))],
        closings: &[(Segment, (&'graph Closing<'graph>, Frontier<'graph>))],
    ) -> Self {
        let units = Keyed::from_sorted(collect(lay, units.iter().copied()));
        let mut held = BumpVec::new_in(scratch);
        for (at, (_, (facts, _))) in units.iter().enumerate() {
            facts.held(&mut |key| held.push((key, at as u32)));
        }
        held.sort_unstable();
        held.dedup();
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
        let computed = (units.iter())
            .flat_map(|(_, (facts, _))| facts.typed.iter())
            .flat_map(|(_, typed)| {
                (typed.contents.iter().map(|(_, tree)| tree))
                    .chain(typed.trees.iter().map(|(_, tree)| *tree))
            });
        Records {
            base,
            binding,
            units,
            closings: Keyed::from_sorted(collect(lay, closings.iter().copied())),
            index: Keyed::from_sorted(collect(lay, held.iter().copied())),
            trees: closed_trees(lay, scratch, computed),
            bodies: &[],
        }
    }

    /// These records, which lie over no base, laid again through `lay` by `relay`, their trees
    /// gathered in `scratch`: what a copy of an activation into another region holds.
    pub(super) fn relaid<'to, L: Lays<'to>>(
        &self,
        lay: L,
        scratch: BumpAllocator<'_>,
        relay: &mut Relay<'to, '_, '_, L>,
    ) -> Records<'to> {
        debug_assert!(self.base.is_none() && self.bodies.is_empty());
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
        let computed = (units.iter())
            .flat_map(|(_, (facts, _))| facts.typed.iter())
            .flat_map(|(_, typed)| {
                (typed.contents.iter().map(|(_, tree)| tree))
                    .chain(typed.trees.iter().map(|(_, tree)| *tree))
            });
        Records {
            base: None,
            binding: Binding::laid(lay, &mut bound),
            units,
            closings: Keyed::from_sorted(collect(lay, closings.iter().copied())),
            index: Keyed::from_sorted(collect(lay, self.index.iter().copied())),
            trees: closed_trees(lay, scratch, computed),
            bodies: &[],
        }
    }

    /// What [`relaid`](Self::relaid) lays of these records' tables, by entry, each record once.
    pub(super) fn weight(&self) -> crate::values::Weight {
        use crate::values::Weight;
        let units = Weight::run::<(UnitId, (&UnitFacts<'_>, Frontier<'_>))>(self.units.len())
            .plus(Weight::run::<UnitFacts<'_>>(self.units.len()));
        let closings = Weight::run::<(Segment, (&Closing<'_>, Frontier<'_>))>(self.closings.len())
            .plus(Weight::run::<Closing<'_>>(self.closings.len()));
        units
            .plus(closings)
            .plus(Weight::run::<(Unknown, Bound<'_>)>(
                self.binding.entries().len(),
            ))
            .plus(Weight::run::<(Held, u32)>(self.index.len()))
    }

    /// No record, under `binding`.
    pub(super) fn bare(binding: Binding<'graph>) -> Self {
        Records {
            base: None,
            binding,
            units: Keyed::EMPTY,
            closings: Keyed::EMPTY,
            index: Keyed::EMPTY,
            trees: Keyed::EMPTY,
            bodies: &[],
        }
    }

    /// Hand `each` every type these records name.
    pub(super) fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for (_, (facts, _)) in self.units.iter() {
            facts.each_type(each);
        }
        for (_, (closing, _)) in self.closings.iter() {
            closing.each_type(each);
        }
    }

    /// What the walk bound.
    pub(super) fn binding(&self) -> Binding<'graph> {
        self.binding
    }

    fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        match self.units.get(unit) {
            Some((_, record)) => Some(*record),
            None => self.base?.0.records.unit(unit),
        }
    }

    fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        match self.closings.get(body) {
            Some((_, record)) => Some(*record),
            None => self.base?.0.records.closing(body),
        }
    }

    /// The record holding `held`, beside its unit: this load's, else its base's where this load did
    /// not re-run the unit, since a re-run record replaces the base's whole.
    fn holding(&self, held: Held) -> Option<(UnitId, &'graph UnitFacts<'graph>)> {
        if let Some((_, at)) = self.index.get(held) {
            let (unit, (facts, _)) = self.units[*at as usize];
            return Some((unit, facts));
        }
        let (unit, facts) = self.base?.0.records.holding(held)?;
        self.units.get(unit).is_none().then_some((unit, facts))
    }

    /// Hand `each` every path entry of the chain `root` roots these records, or their base's, hold.
    fn each_path(&self, root: Segment, each: &mut dyn FnMut(PathEntry<'graph>)) {
        let entries = self.index.entries();
        let from = entries.partition_point(|(held, _)| *held < Held::Path(root, 0));
        let to = entries.partition_point(|(held, _)| *held <= Held::Path(root, usize::MAX));
        for (held, at) in &entries[from..to] {
            let (_, (facts, _)) = self.units[*at as usize];
            if let Held::Path(_, level) = held
                && let Some((_, (entry, _))) = facts.paths.get((root, *level))
            {
                each(*entry);
            }
        }
        if let Some(base) = self.base {
            base.0.records.each_path(root, each);
        }
    }

    /// `body` as this load laid it, or its base where this load has not folded yet.
    fn laid(&self, body: Segment) -> &'graph Laid<'graph> {
        match self.bodies.get(body.index() as usize) {
            Some(laid) => laid,
            None => (self
                .base
                .expect("a load folds its records before a body is read"))
            .0
            .records
            .laid(body),
        }
    }

    /// The folds of `body`: this load's, else its base's.
    fn folds(&self, body: Segment) -> Option<Folds<'graph>> {
        match self
            .bodies
            .get(body.index() as usize)
            .and_then(|laid| laid.folds)
        {
            Some(folds) => Some(folds),
            None => self.base?.0.records.folds(body),
        }
    }
}

impl UnitFacts<'_> {
    /// Each fact this record holds that the shape cannot place, by what it is found by.
    fn held(&self, each: &mut impl FnMut(Held)) {
        for (segment, typed) in self.typed.iter() {
            typed.held(*segment, each);
        }
        for ((root, level), _) in self.paths.iter() {
            each(Held::Path(*root, *level));
        }
    }
}

impl Typed<'_> {
    /// Each site and through fact of `segment` these tables hold, by what it is found by.
    fn held(&self, segment: Segment, each: &mut impl FnMut(Held)) {
        let sites = (self.expressions.iter().map(|(site, _)| *site))
            .chain(self.parts.iter().map(|(site, _)| *site))
            .chain(self.settled.iter().copied())
            .chain(self.instances.iter().map(|(site, _)| *site))
            .chain(self.uses.iter().map(|(site, _)| *site))
            .chain(self.selections.iter().map(|(site, _)| *site))
            .chain(self.by_name.iter().map(|(site, _)| *site))
            .chain(self.routes.iter().map(|(site, _)| *site))
            .chain(self.trees.iter().map(|(site, _)| *site))
            .chain(self.picks.iter().map(|(site, _)| *site))
            .chain(self.minting.iter().copied());
        sites.for_each(|site| each(Held::Site(segment, site)));
    }
}

#[cfg(test)]
thread_local! {
    /// How many rigid facts and points reached the run's door: what the door law holds at nothing.
    /// Tests reset it.
    pub(crate) static RIGID_AT_RUN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count a rigid fact or point the run's door met.
#[cfg(test)]
pub(crate) fn rigid_at_run() {
    RIGID_AT_RUN.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
thread_local! {
    /// How many terms the run probed: each a frame's read found its chain lacking, and each a
    /// walk minted whole before it ran. Tests reset it.
    pub(crate) static TERM_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

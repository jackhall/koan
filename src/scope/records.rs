//! The **load's facts**, read through one door: [`LoadFacts`]. The run and the suites read every
//! fact the load decided about a body through it, each accessor taking the body's shape first, so
//! no reader names where a fact is kept. A shape names its body by its segment alone, so a reader
//! may hand in a shape borrowed for any lifetime.
//!
//! Two backends answer. `Cells` forwards each accessor to the shape's write-once cell the load's
//! passes fixed. `Records` reads the one-walk build's records: one [`UnitFacts`] per unit the walk
//! ran, each beside the frontier it gathered, one [`Closing`] per owning body, and the [`Folds`] the
//! load lays per body after the walk. Every record is `Copy`, names bodies by segment and holds no
//! cell, so each is covariant. A `Records` load may sit over a base: every read tries its own
//! records, then its base's, and a unit it re-ran replaces the base's record of it whole. `Cells`
//! goes once the walk replaces the passes
//! ([the one-walk build](../../roadmap/gradual-build/one-walk-build.md)).
//!
//! A `LoadFacts` is laid in program storage before the program is, and an activation carries it, so
//! every reader holding a view reads through [`ActivationView::load`](super::ActivationView::load).

use crate::memory::{BumpAllocator, BumpVec, Lays, Writer, collect, resident};
use crate::source::SourceRef;
use crate::symbols::TypeSymbol;
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric};
use crate::values::Chain;

use super::facts::{Binding, Frontier, UnitId};
use super::keyed::Keyed;
use super::shape::{BodyShape, Coordinate, Segment, ShapeError, Site, Slot, Target, in_body_of};
use super::shapes::Shapes;
use super::tree::Tree;
use super::typed::{
    BodyClass, Caching, Handoff, InstanceRead, Mint, PathEntry, PathRead, Resolution, Resolved,
    Static, StaticCallable, StaticRegistered, StaticSolution, StaticType, UseStatics, Variable,
    ViewSlots,
};

/// One unit's facts: everything the walk computed running it, the inline bodies it ran included.
#[derive(Clone, Copy)]
pub struct UnitFacts<'a> {
    /// The unit's own body first, then each inline body it ran, by segment.
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
    /// Each path spelling a type expression reads, by site.
    pub reads: Keyed<'a, (Site, PathRead)>,
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
    pub selections: Keyed<'a, (Site, &'a [Option<KType>])>,
    /// Each call by name's contributions, by its argument part's site.
    pub by_name: Keyed<'a, (Site, &'a [StaticType<'a>])>,
    /// Each `:|` at a closed signature, beside its view's content tree.
    pub trees: Keyed<'a, (Site, &'a Tree<'a>)>,
    /// Each binder's content tree, by slot.
    pub contents: Keyed<'a, (Slot, Tree<'a>)>,
    /// The body's own minting sites.
    pub minting: Keyed<'a, Site>,
    /// Where the body reads each lexical variable of a path's `through`, by the path's level.
    pub throughs: Keyed<'a, (usize, &'a [Variable])>,
    /// A `MODULE` or `GROUP` body's exact signature, where the walk typed one.
    pub signature: Option<KType>,
    /// Whether an inline body has a porous exit, and how many sites it mints at run.
    pub exits: bool,
    pub mints: u32,
}

impl Typed<'_> {
    /// A body's facts where a unit fixed none.
    pub const EMPTY: Self = Typed {
        declared: Keyed::EMPTY,
        registered: Keyed::EMPTY,
        expressions: Keyed::EMPTY,
        reads: Keyed::EMPTY,
        parts: Keyed::EMPTY,
        statements: Keyed::EMPTY,
        binders: Keyed::EMPTY,
        settled: Keyed::EMPTY,
        instances: Keyed::EMPTY,
        uses: Keyed::EMPTY,
        selections: Keyed::EMPTY,
        by_name: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        contents: Keyed::EMPTY,
        minting: Keyed::EMPTY,
        throughs: Keyed::EMPTY,
        signature: None,
        exits: false,
        mints: 0,
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
    pub instance: Option<StaticSolution<'a>>,
    /// Its parameters whose arguments digest at a constant cost, sorted: a type binder, or one
    /// declared closed under `Module` or as a type value. The keys and caching folds read it.
    pub digestible: &'a [Slot],
}

/// What the walk computes for an owning body once its units and deferred bodies are typed.
#[derive(Clone, Copy, Debug)]
pub struct Closing<'a> {
    pub class: Option<BodyClass>,
    /// Whether it has a porous exit, its inline bodies' folded in.
    pub exits: bool,
    /// A callable's summary: `Some(None)` where it has none, a recursive one among them.
    pub summary: Option<Option<Tree<'a>>>,
    /// Whether the callable lies in a recursive component: it never unfolds.
    pub recursive: bool,
    /// A quote's code's last statement's static type as a traced `EVAL` runs it unfilled.
    pub ran: Option<Interval<Parametric>>,
    /// Why a quote's code does not type, reported when an `EVAL` runs it.
    pub refusal: Option<&'a ShapeError<'a>>,
}

/// What the load folds per body after the walk.
#[derive(Clone, Copy, Debug)]
pub struct Folds<'a> {
    pub caching: Option<&'a Caching<'a>>,
    /// The parameter slots a callable's specialization keys on: `None` where a call seeks nothing.
    pub keys: Option<&'a [Slot]>,
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
    pub name: TypeSymbol,
    pub at: SourceRef,
}

/// Every fact one load decided, by body, beside the shape table they are facts of.
#[derive(Clone, Copy)]
pub struct LoadFacts<'graph> {
    shapes: &'graph Shapes<'graph>,
    backend: Backend<'graph>,
}

/// A body as a value born of it holds it: its shape, beside which of its type captures its content
/// composes, parallel to [`LoadFacts::type_captures`]. One pointer, so a value holds it in place of
/// its shape's.
#[derive(Clone, Copy)]
pub struct Body<'graph> {
    pub shape: &'graph BodyShape<'graph>,
    pub contract: &'graph [bool],
}

/// Where a [`LoadFacts`] keeps what it answers.
#[derive(Clone, Copy)]
enum Backend<'graph> {
    /// The shapes' own cells, fixed by the load's passes, beside each body by segment. Every type
    /// capture composes, so each body's contract is all `true`.
    Cells {
        bodies: &'graph [Body<'graph>],
    },
    Records(&'graph Records<'graph>),
}

/// One walk's records, over its base's where it has one.
#[derive(Clone, Copy)]
struct Records<'graph> {
    base: Option<&'graph LoadFacts<'graph>>,
    /// What the walk bound: the base's binding and more.
    binding: Binding<'graph>,
    /// Each unit the walk ran, beside its record and the frontier it gathered.
    units: Keyed<'graph, (UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))>,
    /// Each owning body the walk closed.
    closings: Keyed<'graph, (Segment, (&'graph Closing<'graph>, Frontier<'graph>))>,
    /// Which of `units` holds each fact.
    index: Keyed<'graph, (Held, u32)>,
    /// Each body, by segment, beside its type captures as its birth reads them and its folds:
    /// `None` before the load folds, when a read falls to the base, and every capture composes.
    bodies: &'graph [Laid<'graph>],
}

/// One body as a [`Records`] load lays it.
#[derive(Clone, Copy)]
struct Laid<'graph> {
    body: Body<'graph>,
    captures: &'graph [Coordinate],
    folds: Option<Folds<'graph>>,
}

/// A fact a unit's record holds, by what it is found by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Held {
    Site(Segment, Site),
    Slot(Segment, Slot),
    Statement(Segment, u32),
    Through(Segment, usize),
    Born(Segment),
    /// A path entry, by its chain root and level.
    Path(Segment, usize),
}

impl<'graph> LoadFacts<'graph> {
    /// The facts the load's passes fixed into the cells of `shapes`, laid in `writer`'s region.
    pub fn of_cells(writer: Writer<'graph, '_>, shapes: &'graph Shapes<'graph>) -> Self {
        let widest = (shapes.bodies().iter())
            .map(|shape| shape.type_captures().len())
            .max()
            .unwrap_or(0);
        let composes: &[bool] = writer.fill(widest, |_| true);
        let bodies = shapes.bodies();
        let bodies = writer.fill(bodies.len(), |at| Body {
            shape: bodies[at],
            contract: &composes[..bodies[at].type_captures().len()],
        });
        LoadFacts {
            shapes,
            backend: Backend::Cells { bodies },
        }
    }

    /// One walk's records over `shapes`, laid through `lay`, over `base` where the walk resumed
    /// one under `binding`. `folds`, by segment, is empty where the load has not folded the records
    /// yet.
    #[allow(clippy::too_many_arguments)]
    pub fn of_records(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        shapes: &'graph Shapes<'graph>,
        base: Option<&'graph LoadFacts<'graph>>,
        binding: Binding<'graph>,
        units: &mut [(UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))],
        closings: &mut [(Segment, (&'graph Closing<'graph>, Frontier<'graph>))],
        folds: &[Folds<'graph>],
    ) -> Self {
        debug_assert!(base.is_none_or(|base| binding.extends(base.binding())));
        let units = Keyed::sorted(lay, units);
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
        let records = Records {
            base,
            binding,
            units,
            closings: Keyed::sorted(lay, closings),
            index: Keyed::from_sorted(collect(lay, held.iter().copied())),
            bodies: &[],
        };
        LoadFacts {
            shapes,
            backend: Backend::Records(resident(lay, records)),
        }
        .folded(lay, folds)
    }

    /// These records beside `folds`, by segment: none where it is empty.
    pub fn folded(&self, lay: impl Lays<'graph>, folds: &[Folds<'graph>]) -> Self {
        let Backend::Records(records) = self.backend else {
            panic!("only a walk's records fold");
        };
        let shapes = self.shapes;
        debug_assert!(folds.is_empty() || folds.len() == shapes.bodies().len());
        let bodies = shapes.bodies();
        let all: &[bool] = lay.fill(
            (bodies
                .iter()
                .map(|shape| shapes.layout(shape.segment()).len()))
            .max()
            .unwrap_or(0),
            |_| true,
        );
        let bodies = lay.fill(bodies.len(), |at| {
            let shape = bodies[at];
            let layout = shapes.layout(shape.segment());
            let folds = folds.get(at).copied();
            Laid {
                body: Body {
                    shape,
                    contract: folds.map_or(&all[..layout.len()], |folds| folds.contract),
                },
                captures: collect(lay, layout.iter().map(|(_, coordinate)| *coordinate)),
                folds,
            }
        });
        LoadFacts {
            shapes,
            backend: Backend::Records(resident(lay, Records { bodies, ..*records })),
        }
    }

    /// What the load's walk bound: nothing under `Cells`, or at the load.
    pub fn binding(&self) -> Binding<'graph> {
        match self.backend {
            Backend::Cells { .. } => Binding::EMPTY,
            Backend::Records(records) => records.binding,
        }
    }

    /// The load this one resumed, where it resumed one.
    pub fn base(&self) -> Option<&'graph LoadFacts<'graph>> {
        match self.backend {
            Backend::Cells { .. } => None,
            Backend::Records(records) => records.base,
        }
    }

    /// The newest record of `unit` beside its frontier, its base's where this load did not re-run
    /// it: `None` under `Cells`, which keeps none.
    pub fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        match self.backend {
            Backend::Cells { .. } => None,
            Backend::Records(records) => records.unit(unit),
        }
    }

    /// The newest closing record of the owning body `body`, beside its frontier.
    pub fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        match self.backend {
            Backend::Cells { .. } => None,
            Backend::Records(records) => records.closing(body),
        }
    }

    /// The units this load ran itself, each beside its record and frontier, sorted.
    pub fn own_units(&self) -> &'graph [(UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))] {
        match self.backend {
            Backend::Cells { .. } => &[],
            Backend::Records(records) => records.units.entries(),
        }
    }

    /// The shape table these are facts of.
    pub fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    /// `shape` as this load's table holds it, found by its segment: so a reader's shape may be
    /// borrowed for any lifetime.
    fn at(&self, shape: &BodyShape<'_>) -> &'graph BodyShape<'graph> {
        let at = self.shapes.shape_at(shape.segment());
        debug_assert!(std::ptr::addr_eq(at, shape), "a shape of this load");
        at
    }

    /// What the load fixed for the type expression at `site` of `shape`: `Unknown` where it records
    /// none there.
    pub fn typed_expression(&self, shape: &BodyShape<'_>, site: Site) -> StaticType<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).typed_expression(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.expressions.get(site))
            .map_or(Static::Unknown, |(_, typed)| *typed),
        }
    }

    /// What the load fixed for the type binder at `slot` of `shape`.
    pub fn declared_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticType<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).declared_type(slot),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Slot(shape.segment(), slot)))
            .and_then(|typed| typed.declared.get(slot))
            .map_or(Static::Unknown, |(_, declared)| *declared),
        }
    }

    /// What the load fixed for the registration at `slot` of `shape`.
    pub fn registered_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticRegistered<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).registered_type(slot),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Slot(shape.segment(), slot)))
            .and_then(|typed| typed.registered.get(slot))
            .map_or(Static::Unknown, |(_, registered)| *registered),
        }
    }

    /// What the load fixed for the callable body `shape`'s type.
    pub fn callable_type(&self, shape: &BodyShape<'_>) -> StaticCallable<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).callable_type(),
            Backend::Records(records) => {
                (records.born(shape.segment())).map_or(Static::Unknown, |born| born.callable)
            }
        }
    }

    /// The callable body `shape`'s own `FOR ALL` group as its body reads it, by group order.
    pub fn group_levels(&self, shape: &BodyShape<'_>) -> &'graph [Parametric] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).group_levels(),
            Backend::Records(records) => records
                .born(shape.segment())
                .map_or(&[], |born| born.levels),
        }
    }

    /// The callable body `shape`'s function type as its body reads it: `None` where the load did
    /// not type it.
    pub fn in_body(
        &self,
        shape: &BodyShape<'_>,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Option<Parametric> {
        in_body_of(
            self.callable_type(shape),
            self.group_levels(shape),
            types,
            scratch,
        )
    }

    /// The solution the callable body `shape`'s `FOR ALL` is born instantiated at, where the load
    /// solved one.
    pub fn born_instance(&self, shape: &BodyShape<'_>) -> Option<StaticSolution<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).born_instance(),
            Backend::Records(records) => records.born(shape.segment())?.instance,
        }
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub fn homes(&self, shape: &BodyShape<'_>) -> &'graph [(usize, Target)] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).declared_variables(),
            Backend::Records(_) => self.shapes.homes(shape.segment()),
        }
    }

    /// Each type capture of `shape` past its builder captures, as the coordinate its birth reads.
    pub fn type_captures(&self, shape: &BodyShape<'_>) -> &'graph [Coordinate] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).type_captures(),
            Backend::Records(records) => records.bodies[shape.segment().index() as usize].captures,
        }
    }

    /// How many closure slots a birth of `shape` fills: its builder captures, then its type
    /// captures.
    pub fn capture_count(&self, shape: &BodyShape<'_>) -> usize {
        shape.captures().len() + self.type_captures(shape).len()
    }

    /// `shape` as a value born of it holds it.
    pub fn body(&self, shape: &BodyShape<'_>) -> &'graph Body<'graph> {
        match self.backend {
            Backend::Cells { bodies } => &bodies[shape.segment().index() as usize],
            Backend::Records(records) => &records.bodies[shape.segment().index() as usize].body,
        }
    }

    /// Which of `shape`'s type captures its content composes, parallel to
    /// [`type_captures`](Self::type_captures).
    pub fn contract(&self, shape: &BodyShape<'_>) -> &'graph [bool] {
        self.body(shape).contract
    }

    /// The path spelling `shape`'s type expressions read at `site`, where they read one.
    pub fn path_read(&self, shape: &BodyShape<'_>, site: Site) -> Option<PathRead> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).path_read(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.reads.get(site))
            .map(|(_, read)| *read),
        }
    }

    /// The path entry at `level` of the chain the body `root` roots.
    pub fn path_entry(&self, root: &BodyShape<'_>, level: usize) -> Option<PathEntry<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).path_entry(level),
            Backend::Records(records) => {
                records.path(root.segment(), level).map(|(entry, _)| entry)
            }
        }
    }

    /// What the load resolved the path at `level` of the chain `root` roots to.
    pub fn resolution(&self, root: &BodyShape<'_>, level: usize) -> Option<Resolution<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).resolution(level),
            Backend::Records(records) => records
                .path(root.segment(), level)
                .map(|(_, resolution)| resolution),
        }
    }

    /// The type the path at `level` of the chain `root` roots is at every run, where the load
    /// resolved it to one.
    pub fn path_closed(&self, root: &BodyShape<'_>, level: usize) -> Option<KType> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).path_closed(level),
            Backend::Records(_) => match self.resolution(root, level)?.resolved {
                Resolved::Closed(closed) => Some(closed),
                Resolved::Class(_) => None,
            },
        }
    }

    /// Where `shape` reads each lexical variable of the path at `level`'s `through`.
    pub fn through_at(&self, shape: &BodyShape<'_>, level: usize) -> Option<&'graph [Variable]> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).through_at(level),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Through(shape.segment(), level)))
            .and_then(|typed| typed.throughs.get(level))
            .map(|(_, located)| *located),
        }
    }

    /// Why the code shape `shape`'s code cannot be built or does not type, reported when an `EVAL`
    /// runs it.
    pub fn typing_refusal(&self, shape: &BodyShape<'_>) -> Option<&'graph ShapeError<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).refusal(),
            Backend::Records(records) => records.closing(shape.segment())?.0.refusal,
        }
    }

    /// The static type of the part at `site` of `shape`, where the load typed one.
    pub fn value_type(&self, shape: &BodyShape<'_>, site: Site) -> Option<Interval> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).value_type(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.parts.get(site))
            .map(|(_, typed)| *typed),
        }
    }

    /// The static type of statement `index` of `shape`'s body.
    pub fn statement_type(&self, shape: &BodyShape<'_>, index: usize) -> Option<Interval> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).statement_type(index),
            Backend::Records(records) => {
                let index = index as u32;
                (records.typed(shape.segment(), Held::Statement(shape.segment(), index)))
                    .and_then(|typed| typed.statements.get(index))
                    .map(|(_, typed)| *typed)
            }
        }
    }

    /// The static type of the binder at `slot` of `shape`.
    pub fn binder_type(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<DeclaredType<Interval>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).binder_type(slot),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Slot(shape.segment(), slot)))
            .and_then(|typed| typed.binders.get(slot))
            .map(|(_, typed)| *typed),
        }
    }

    /// What the load fixed about the keyworded use whose node sits at `site` of `shape`.
    pub fn use_statics(&self, shape: &BodyShape<'_>, site: Site) -> UseStatics<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).use_statics(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.uses.get(site))
            .map_or(UseStatics::UNFIXED, |(_, fixed)| *fixed),
        }
    }

    /// The class the load fixed for the frame body `shape`.
    pub fn class(&self, shape: &BodyShape<'_>) -> Option<BodyClass> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).class(),
            Backend::Records(records) => records.closing(shape.segment())?.0.class,
        }
    }

    /// Whether the frame body `shape` caches.
    pub fn caches(&self, shape: &BodyShape<'_>) -> bool {
        self.caching(shape).is_some_and(|caching| caching.caches)
    }

    /// Where `shape`'s types intern at run.
    pub fn caching(&self, shape: &BodyShape<'_>) -> Option<&'graph Caching<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).caching(),
            Backend::Records(records) => records.folds(shape.segment())?.caching,
        }
    }

    /// How the load classed the minting site at `site`, one of `shape`'s own.
    pub fn mint_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<Mint> {
        let caching = self.caching(shape)?;
        caching.mints.get(site).map(|(_, mint)| *mint)
    }

    /// Whether the load recorded `site` as one of `shape`'s own minting sites.
    pub fn mints_at(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).mints_at(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .is_some_and(|typed| typed.minting.get(site).is_some()),
        }
    }

    /// Whether the composite site at `site` of `shape` builds a value whose own type is warm.
    pub fn warm_at(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        (self.caching(shape)).is_some_and(|caching| caching.warm.get(site).is_some())
    }

    /// The content tree of the view the `:|` at `site` of `shape` builds, where its signature is
    /// closed.
    pub fn view_tree(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'graph Tree<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).view_tree(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.trees.get(site))
            .map(|(_, tree)| *tree),
        }
    }

    /// Whether the load settled the ascription at `site` of `shape`.
    pub fn settled(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).settled(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .is_some_and(|typed| typed.settled.get(site).is_some()),
        }
    }

    /// What the load fixed where `shape` reads a quantified function at `site`.
    pub fn instance_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<InstanceRead<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).instance_at(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.instances.get(site))
            .map(|(_, read)| *read),
        }
    }

    /// What each parameter of the call by name whose argument sits at `site` of `shape` is solved
    /// from.
    pub fn named_contributions(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> &'graph [StaticType<'graph>] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).named_contributions(site),
            Backend::Records(records) => (records
                .typed(shape.segment(), Held::Site(shape.segment(), site)))
            .and_then(|typed| typed.by_name.get(site))
            .map_or(&[], |(_, recorded)| *recorded),
        }
    }

    /// The parameter slots the callable `shape`'s specialization keys on: `None` where a call of
    /// it seeks nothing.
    pub fn specialization_keys(&self, shape: &BodyShape<'_>) -> Option<&'graph [Slot]> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).specialization_keys(),
            Backend::Records(records) => records.folds(shape.segment())?.keys,
        }
    }

    /// How many of `shape`'s slots keep a digest memo.
    pub fn digested_count(&self, shape: &BodyShape<'_>) -> usize {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).digested_count(),
            Backend::Records(_) => self.shapes.digested_count(shape.segment()),
        }
    }

    /// Where `slot`'s digest memo sits among `shape`'s, if it keeps one.
    pub fn digested_index(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<usize> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).digested_index(slot),
            Backend::Records(_) => self.shapes.digested_index(shape.segment(), slot),
        }
    }

    /// Where the captures of the module the `:|` at `site` of `shape` views keep their memos.
    pub fn view_slots(&self, shape: &BodyShape<'_>, site: Site) -> Option<ViewSlots<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).view_slots(site),
            Backend::Records(_) => self.shapes.view_slots(shape.segment(), site),
        }
    }

    /// The digested slots the call keyed by `site` of `shape` passes bare.
    pub fn handoffs(&self, shape: &BodyShape<'_>, site: Site) -> &'graph [Handoff] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).handoffs(site),
            Backend::Records(_) => self.shapes.handoffs(shape.segment(), site),
        }
    }
}

impl<'graph> Records<'graph> {
    fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        match self.units.get(unit) {
            Some((_, record)) => Some(*record),
            None => self.base?.unit(unit),
        }
    }

    fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        match self.closings.get(body) {
            Some((_, record)) => Some(*record),
            None => self.base?.closing(body),
        }
    }

    /// The record holding `held`, beside its unit: this load's, else its base's where this load did
    /// not re-run the unit, since a re-run record replaces the base's whole.
    fn holding(&self, held: Held) -> Option<(UnitId, &'graph UnitFacts<'graph>)> {
        if let Some((_, at)) = self.index.get(held) {
            let (unit, (facts, _)) = self.units[*at as usize];
            return Some((unit, facts));
        }
        let Backend::Records(base) = self.base?.backend else {
            return None;
        };
        let (unit, facts) = base.holding(held)?;
        self.units.get(unit).is_none().then_some((unit, facts))
    }

    /// `segment`'s facts in the record holding `held`.
    fn typed(&self, segment: Segment, held: Held) -> Option<&'graph Typed<'graph>> {
        let (_, facts) = self.holding(held)?;
        facts.typed.get(segment).map(|(_, typed)| typed)
    }

    fn born(&self, body: Segment) -> Option<&'graph Born<'graph>> {
        let (_, facts) = self.holding(Held::Born(body))?;
        facts.born.get(body).map(|(_, born)| born)
    }

    /// The path entry at `level` of the chain `root` roots, beside its resolution.
    fn path(&self, root: Segment, level: usize) -> Option<(PathEntry<'graph>, Resolution<'graph>)> {
        let (_, facts) = self.holding(Held::Path(root, level))?;
        facts.paths.get((root, level)).map(|(_, path)| *path)
    }

    /// The folds of `body`: this load's, else its base's.
    fn folds(&self, body: Segment) -> Option<Folds<'graph>> {
        match self
            .bodies
            .get(body.index() as usize)
            .and_then(|laid| laid.folds)
        {
            Some(folds) => Some(folds),
            None => match self.base?.backend {
                Backend::Records(base) => base.folds(body),
                Backend::Cells { .. } => None,
            },
        }
    }
}

impl UnitFacts<'_> {
    /// Each fact this record holds, by what it is found by.
    fn held(&self, each: &mut impl FnMut(Held)) {
        for (segment, typed) in self.typed.iter() {
            typed.held(*segment, each);
        }
        for (segment, _) in self.born.iter() {
            each(Held::Born(*segment));
        }
        for ((root, level), _) in self.paths.iter() {
            each(Held::Path(*root, *level));
        }
    }
}

impl Typed<'_> {
    /// Each fact of `segment` these tables hold, by what it is found by.
    fn held(&self, segment: Segment, each: &mut impl FnMut(Held)) {
        let sites = (self.expressions.iter().map(|(site, _)| *site))
            .chain(self.reads.iter().map(|(site, _)| *site))
            .chain(self.parts.iter().map(|(site, _)| *site))
            .chain(self.settled.iter().copied())
            .chain(self.instances.iter().map(|(site, _)| *site))
            .chain(self.uses.iter().map(|(site, _)| *site))
            .chain(self.selections.iter().map(|(site, _)| *site))
            .chain(self.by_name.iter().map(|(site, _)| *site))
            .chain(self.trees.iter().map(|(site, _)| *site))
            .chain(self.minting.iter().copied());
        sites.for_each(|site| each(Held::Site(segment, site)));
        let slots = (self.declared.iter().map(|(slot, _)| *slot))
            .chain(self.registered.iter().map(|(slot, _)| *slot))
            .chain(self.binders.iter().map(|(slot, _)| *slot))
            .chain(self.contents.iter().map(|(slot, _)| *slot));
        slots.for_each(|slot| each(Held::Slot(segment, slot)));
        for (index, _) in self.statements.iter() {
            each(Held::Statement(segment, *index));
        }
        for (level, _) in self.throughs.iter() {
            each(Held::Through(segment, *level));
        }
    }
}

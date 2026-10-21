//! The **load's facts**, read through one door: [`LoadFacts`]. The run and the suites read every
//! fact the load decided about a body through it, each accessor taking the body's shape first, so
//! no reader names where a fact is kept. A shape names its body by its segment alone, so a reader
//! may hand in a shape borrowed for any lifetime.
//!
//! Today one backend answers: `Cells`, which forwards each accessor to the shape's write-once cell
//! the load's passes fixed. The one-walk build adds a second, which reads per-unit records instead,
//! and then retires the cells ([the one-walk build](../../roadmap/gradual-build/one-walk-build.md)).
//!
//! A `LoadFacts` is laid in program storage before the program is, and an activation carries it, so
//! every reader holding a view reads through [`ActivationView::load`](super::ActivationView::load).

use crate::memory::{BumpAllocator, Writer};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric};
use crate::values::Chain;

use super::shape::{BodyShape, Coordinate, ShapeError, Site, Slot, Target, in_body_of};
use super::shapes::Shapes;
use super::tree::Tree;
use super::typed::{
    BodyClass, Caching, Handoff, InstanceRead, Mint, PathEntry, PathRead, Resolution,
    StaticCallable, StaticRegistered, StaticSolution, StaticType, UseStatics, Variable, ViewSlots,
};

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
    Cells { bodies: &'graph [Body<'graph>] },
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
        }
    }

    /// What the load fixed for the type binder at `slot` of `shape`.
    pub fn declared_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticType<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).declared_type(slot),
        }
    }

    /// What the load fixed for the registration at `slot` of `shape`.
    pub fn registered_type(&self, shape: &BodyShape<'_>, slot: Slot) -> StaticRegistered<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).registered_type(slot),
        }
    }

    /// What the load fixed for the callable body `shape`'s type.
    pub fn callable_type(&self, shape: &BodyShape<'_>) -> StaticCallable<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).callable_type(),
        }
    }

    /// The callable body `shape`'s own `FOR ALL` group as its body reads it, by group order.
    pub fn group_levels(&self, shape: &BodyShape<'_>) -> &'graph [Parametric] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).group_levels(),
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
        }
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub fn homes(&self, shape: &BodyShape<'_>) -> &'graph [(usize, Target)] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).declared_variables(),
        }
    }

    /// Each type capture of `shape` past its builder captures, as the coordinate its birth reads.
    pub fn type_captures(&self, shape: &BodyShape<'_>) -> &'graph [Coordinate] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).type_captures(),
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
        }
    }

    /// The path entry at `level` of the chain the body `root` roots.
    pub fn path_entry(&self, root: &BodyShape<'_>, level: usize) -> Option<PathEntry<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).path_entry(level),
        }
    }

    /// What the load resolved the path at `level` of the chain `root` roots to.
    pub fn resolution(&self, root: &BodyShape<'_>, level: usize) -> Option<Resolution<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).resolution(level),
        }
    }

    /// The type the path at `level` of the chain `root` roots is at every run, where the load
    /// resolved it to one.
    pub fn path_closed(&self, root: &BodyShape<'_>, level: usize) -> Option<KType> {
        match self.backend {
            Backend::Cells { .. } => self.at(root).path_closed(level),
        }
    }

    /// Where `shape` reads each lexical variable of the path at `level`'s `through`.
    pub fn through_at(&self, shape: &BodyShape<'_>, level: usize) -> Option<&'graph [Variable]> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).through_at(level),
        }
    }

    /// Why the code shape `shape`'s code cannot be built or does not type, reported when an `EVAL`
    /// runs it.
    pub fn typing_refusal(&self, shape: &BodyShape<'_>) -> Option<&'graph ShapeError<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).refusal(),
        }
    }

    /// The static type of the part at `site` of `shape`, where the load typed one.
    pub fn value_type(&self, shape: &BodyShape<'_>, site: Site) -> Option<Interval> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).value_type(site),
        }
    }

    /// The static type of statement `index` of `shape`'s body.
    pub fn statement_type(&self, shape: &BodyShape<'_>, index: usize) -> Option<Interval> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).statement_type(index),
        }
    }

    /// The static type of the binder at `slot` of `shape`.
    pub fn binder_type(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<DeclaredType<Interval>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).binder_type(slot),
        }
    }

    /// What the load fixed about the keyworded use whose node sits at `site` of `shape`.
    pub fn use_statics(&self, shape: &BodyShape<'_>, site: Site) -> UseStatics<'graph> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).use_statics(site),
        }
    }

    /// The class the load fixed for the frame body `shape`.
    pub fn class(&self, shape: &BodyShape<'_>) -> Option<BodyClass> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).class(),
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
        }
    }

    /// Whether the load settled the ascription at `site` of `shape`.
    pub fn settled(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).settled(site),
        }
    }

    /// What the load fixed where `shape` reads a quantified function at `site`.
    pub fn instance_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<InstanceRead<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).instance_at(site),
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
        }
    }

    /// The parameter slots the callable `shape`'s specialization keys on: `None` where a call of
    /// it seeks nothing.
    pub fn specialization_keys(&self, shape: &BodyShape<'_>) -> Option<&'graph [Slot]> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).specialization_keys(),
        }
    }

    /// How many of `shape`'s slots keep a digest memo.
    pub fn digested_count(&self, shape: &BodyShape<'_>) -> usize {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).digested_count(),
        }
    }

    /// Where `slot`'s digest memo sits among `shape`'s, if it keeps one.
    pub fn digested_index(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<usize> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).digested_index(slot),
        }
    }

    /// Where the captures of the module the `:|` at `site` of `shape` views keep their memos.
    pub fn view_slots(&self, shape: &BodyShape<'_>, site: Site) -> Option<ViewSlots<'graph>> {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).view_slots(site),
        }
    }

    /// The digested slots the call keyed by `site` of `shape` passes bare.
    pub fn handoffs(&self, shape: &BodyShape<'_>, site: Site) -> &'graph [Handoff] {
        match self.backend {
            Backend::Cells { .. } => self.at(shape).handoffs(site),
        }
    }
}

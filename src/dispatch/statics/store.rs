//! The static pass's **fact store**: the one door every read and write of a load-time fact goes
//! through ([the build module's design](../../build/README.md#type-facts-and-the-solver)).
//!
//! A fact is a type channel cell the pass reads — a type expression's, a type binder's, a
//! registration's, a callable's — or one the pass fixes itself: a part's, a statement's or a
//! binder's static type, a use's narrowing, a shape's summary or signature. The rest of the pass
//! reaches none of them but through the methods here, which `tests::boundary` holds it to: so
//! what a rule reads is what the store hands it, and the store is the one place a read is
//! recorded.

use crate::memory::{BumpVec, collect, resident};
use crate::scope::{
    BodyShape, Caching, Coordinate, Keyed, Narrowing, PathEntry, PathRead, Resolution, ShapeFacts,
    Site, Slot, StaticCallable, StaticRegistered, StaticSolution, StaticType, Statics, Target,
    UseStatics,
};
use crate::type_lattice::{Interval, KType, Parametric};

use super::{Bound, Level, Pass};

/// What the pass resolved `entry` to, once it has: read once the walk ends, by the caching
/// fixpoint alone.
pub(super) fn resolution_of<'graph>(entry: &PathEntry<'graph>) -> Option<Resolution<'graph>> {
    entry.resolution()
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
    /// The type channel's cell for the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'graph> {
        shape.typed_expression(site)
    }

    /// The type channel's cell for the type binder at `slot` of `shape`.
    pub(super) fn declared_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        shape.declared_type(slot)
    }

    /// The type channel's cell for the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'graph> {
        shape.registered_type(slot)
    }

    /// The type channel's cell for the callable body `body`'s type.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'graph> {
        body.callable_type()
    }

    /// The function type of the callable body `body` as its body reads it, its own group at its
    /// group levels: `None` where the load did not type the callable.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>) -> Option<Parametric> {
        body.in_body(self.types, self.scratch)
    }

    /// The function type of the callable body at `level` as its body reads it: its own group
    /// instantiated at its group levels, its paths read through their resolutions. `None` where
    /// the load did not type the callable, or a path it reads is not resolved.
    pub(super) fn in_body(&self, level: usize) -> Option<Parametric> {
        let shape = self.chain[level].shape;
        let ktype = self.body_type(shape)?;
        match self.callable_cell(shape).over_paths() {
            true => self.through_paths(level, ktype),
            false => Some(ktype),
        }
    }

    /// Whether the quote's code `code` holds a refusal: the builder's, or the type channel's.
    pub(super) fn code_refused(&self, code: &BodyShape<'graph>) -> bool {
        code.refusal().is_some()
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub(super) fn homes(&self, shape: &BodyShape<'graph>) -> &'graph [(usize, Target)] {
        shape.declared_variables()
    }

    /// Each path spelling `shape`'s type expressions read, by site.
    pub(super) fn spellings(
        &self,
        shape: &BodyShape<'graph>,
    ) -> &'graph [(Site, PathRead<'graph>)] {
        shape.path_reads()
    }

    /// The path spelling `shape`'s type expressions read at `site`, where they read one.
    pub(super) fn spelling(
        &self,
        shape: &BodyShape<'graph>,
        site: Site,
    ) -> Option<PathRead<'graph>> {
        shape.path_read(site)
    }

    /// The path entries the type channel minted for the chain `root` roots.
    pub(super) fn channel_entries(
        &self,
        root: &BodyShape<'graph>,
    ) -> &'graph [&'graph PathEntry<'graph>] {
        root.paths()
    }

    /// The type captures the pass laid on `body`.
    pub(super) fn laid_captures(&self, body: &BodyShape<'graph>) -> &'graph [Coordinate] {
        body.type_captures()
    }

    /// The solution the callable body `body` is born instantiated at, where the pass fixed one.
    pub(super) fn born_instance(&self, body: &BodyShape<'graph>) -> Option<StaticSolution<'graph>> {
        body.born_instance()
    }

    /// Whether the pass has fixed `shape`'s cell.
    pub(super) fn typed_shape(&self, shape: &BodyShape<'graph>) -> bool {
        shape.statics().is_some()
    }

    /// Whether the pass has typed `nested`, or need not: a quote's code the load refused is left.
    pub(super) fn done(&self, nested: &BodyShape<'graph>) -> bool {
        self.typed_shape(nested)
            || (nested.kind() == crate::scope::ShapeKind::Code && self.code_refused(nested))
    }

    /// The static type of the binder at `slot` of the shape at `level`, as the pass has it now.
    pub(super) fn binder_at(&self, level: usize, slot: Slot) -> Bound {
        self.chain[level].binders[slot.index()]
    }

    /// Fix the static type of the binder at `slot` of the shape at `level`.
    pub(super) fn set_binder(&mut self, level: usize, slot: Slot, bound: Bound) {
        self.chain[level].binders[slot.index()] = bound;
    }

    /// The static type of statement `index` of the shape at `level`, as the pass has it now.
    pub(super) fn statement_at(&self, level: usize, index: usize) -> Interval {
        self.chain[level].statements[index]
    }

    /// The static type of the last statement of the shape at `level`, where it has one.
    pub(super) fn last_statement(&self, level: usize) -> Option<Interval> {
        self.chain[level].statements.last().copied()
    }

    /// Fix the static type of statement `index` of the shape at `level`.
    pub(super) fn set_statement(&mut self, level: usize, index: usize, typed: Interval) {
        self.chain[level].statements[index] = typed;
    }

    /// The static type the pass recorded for the part at `site` of the shape at `level`, its
    /// latest where it recorded more than one.
    pub(super) fn part_type(&self, level: usize, site: Site) -> Option<Interval> {
        (self.chain[level].parts.iter().rev())
            .find(|(at, _)| *at == site)
            .map(|(_, typed)| *typed)
    }

    /// Record the static type of the part at `site` of the shape at `level`.
    pub(super) fn record_part(&mut self, level: usize, site: Site, typed: Interval) {
        self.chain[level].parts.push((site, typed));
    }

    /// What the pass fixed about the keyworded use at `index` of the shape at `level`.
    pub(super) fn use_at(&self, level: usize, index: usize) -> UseStatics<'graph> {
        self.chain[level].uses[index]
    }

    /// Fix the narrowing of the keyworded use at `index` of the shape at `level`.
    pub(super) fn set_narrowing(
        &mut self,
        level: usize,
        index: usize,
        narrowing: Narrowing<'graph>,
    ) {
        self.chain[level].uses[index].narrowing = narrowing;
    }

    /// Fix the contributions of the keyworded use at `index` of the shape at `level`.
    pub(super) fn set_contributions(
        &mut self,
        level: usize,
        index: usize,
        contributions: &'graph [StaticType<'graph>],
    ) {
        self.chain[level].uses[index].contributions = contributions;
    }

    /// The closed point each variable of the candidate selected at the use at `site` of the shape
    /// at `level` solves to, where the pass selected one there.
    pub(super) fn selection_at(&self, level: usize, site: Site) -> Option<&'graph [Option<KType>]> {
        (self.chain[level].selections.iter())
            .find(|(at, _)| *at == site)
            .map(|(_, points)| *points)
    }

    /// Record the points the candidate selected at the use at `site` solves to.
    pub(super) fn record_selection(
        &mut self,
        level: usize,
        site: Site,
        points: &'graph [Option<KType>],
    ) {
        self.chain[level].selections.push((site, points));
    }

    /// Each binder's static type in `popped`, a shape the pass typed and took off the chain.
    pub(super) fn binders_of<'l>(popped: &'l Level<'p, 'graph>) -> &'l [Bound] {
        &popped.binders
    }

    /// Whether the shape at `level` locates the lexical variables of the path at `path_level`'s
    /// application already.
    pub(super) fn has_through(&self, level: usize, path_level: usize) -> bool {
        (self.chain[level].throughs.iter()).any(|(held, _)| *held == path_level)
    }

    /// Record where the shape at `level` reads each lexical variable of the path at
    /// `path_level`'s application.
    pub(super) fn record_through(
        &mut self,
        level: usize,
        path_level: usize,
        located: &'graph [crate::scope::Variable<'graph>],
    ) {
        self.chain[level].throughs.push((path_level, located));
    }

    /// Record the content tree of the view the `:|` at `site` builds.
    pub(super) fn record_view_tree(
        &mut self,
        level: usize,
        site: Site,
        tree: &'graph crate::scope::Tree<'graph>,
    ) {
        self.chain[level].trees.push((site, tree));
    }

    /// Record that the ascription or annotated binder's type part at `site` is settled.
    pub(super) fn record_settled(&mut self, level: usize, site: Site) {
        self.chain[level].settled.push(site);
    }

    /// Record what the pass fixed where a quantified function is read at `site`.
    pub(super) fn record_instance_read(
        &mut self,
        level: usize,
        site: Site,
        read: crate::scope::InstanceRead<'graph>,
    ) {
        self.chain[level].instances.push((site, read));
    }

    /// Record a call by name's contributions by its argument part's site.
    pub(super) fn record_named(
        &mut self,
        level: usize,
        site: Site,
        contributions: &'graph [StaticType<'graph>],
    ) {
        self.chain[level].named.push((site, contributions));
    }

    /// Fix the solution the callable body `body` is born instantiated at.
    pub(super) fn fix_born_instance(
        &self,
        body: &BodyShape<'graph>,
        solution: StaticSolution<'graph>,
    ) {
        body.fix_born_instance(solution);
    }

    /// Lay the type captures the pass added to `shape` down.
    pub(super) fn fix_type_captures(
        &self,
        shape: &BodyShape<'graph>,
        captures: &'graph [Coordinate],
    ) {
        shape.fix_type_captures(captures);
    }

    /// Lay what the pass typed of the shape at `level` down in its cell.
    pub(super) fn fix(&mut self, level: usize) {
        if self.unfilled {
            return;
        }
        let (writer, scratch) = (self.writer, self.scratch);
        let class = self.class(level);
        let at = &mut self.chain[level];
        let mut uses = BumpVec::new_in(scratch);
        uses.extend(
            (at.shape.candidate_lists().iter())
                .zip(at.uses.iter())
                .filter(|(_, fixed)| !fixed.is_unfixed())
                .map(|((site, _), fixed)| (*site, *fixed)),
        );
        at.digested.sort_unstable();
        at.digested.dedup();
        let shape = at.shape;
        let statics = Statics {
            parts: Keyed::sorted(writer, &mut at.parts),
            statements: collect(writer, at.statements.iter().copied()),
            binders: collect(writer, at.binders.iter().copied()),
            uses: Keyed::sorted(writer, &mut uses),
            selections: Keyed::sorted(writer, &mut at.selections),
            settled: Keyed::sorted(writer, &mut at.settled),
            instances: Keyed::sorted(writer, &mut at.instances),
            named: Keyed::sorted(writer, &mut at.named),
            class,
            minting: Keyed::sorted(writer, &mut at.minting),
            trees: Keyed::sorted(writer, &mut at.trees),
            throughs: Keyed::sorted(writer, &mut at.throughs),
            digested: collect(writer, at.digested.iter().copied()),
            view_slots: Keyed::sorted(writer, &mut at.view_slots),
            handoffs: Keyed::sorted(writer, &mut at.handoffs),
        };
        self.fix_statics(shape, statics);
    }

    /// Lay `shape`'s cell down.
    pub(super) fn fix_statics(&self, shape: &BodyShape<'graph>, statics: Statics<'graph>) {
        debug_assert!(
            statics
                .parts
                .windows(2)
                .all(|pair| pair[0].0 != pair[1].0 || pair[0].1 == pair[1].1),
            "a part typed twice is typed alike"
        );
        shape.fix_statics(statics);
    }

    /// Write the static pass's facts about `shape` as a whole.
    pub(super) fn fix_shape_facts(
        &self,
        shape: &BodyShape<'graph>,
        edit: impl FnOnce(&mut ShapeFacts<'graph>),
    ) {
        shape.fix_shape_facts(edit);
    }

    /// Keep the refusal of the quote's code `code`.
    pub(super) fn refuse_code(
        &self,
        code: &BodyShape<'graph>,
        refusal: &'graph crate::scope::ShapeError<'graph>,
    ) {
        code.refuse_typing(refusal);
    }

    /// Lay where `holder`'s types intern at run down.
    pub(super) fn fix_caching(&self, holder: &BodyShape<'graph>, caching: Caching<'graph>) {
        holder.fix_caching(resident(self.writer, caching));
    }

    /// Fix what the path `entry` resolved to.
    pub(super) fn fix_resolution(&self, entry: &PathEntry<'graph>, resolution: Resolution<'graph>) {
        entry.fix_resolution(resolution);
    }

    /// Lay every path entry of the chain `root` roots down, the type channel's first.
    pub(super) fn fix_paths(
        &self,
        root: &BodyShape<'graph>,
        paths: &'graph [&'graph PathEntry<'graph>],
    ) {
        root.fix_paths(paths);
    }
}

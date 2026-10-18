//! The static pass's **fact store**: the one door every read and write of a load-time fact goes
//! through ([the build module's design](../../build/README.md#type-facts-and-the-solver)).
//!
//! A fact is a type channel cell the pass reads — a type expression's, a type binder's, a
//! registration's, a callable's — or one the pass fixes itself: a part's, a statement's or a
//! binder's static type, a use's narrowing, a shape's summary or signature. The rest of the pass
//! reaches none of them but through the methods here, which `tests::boundary` holds it to: so
//! what a rule reads is what the store hands it, and the store is the one place a read is
//! recorded.
//!
//! Each rule run gathers a **frontier**: the [`Unknown`]s every fact it read waits on, and each
//! unknown it read itself — a lexical variable's home, a type binder the type channel left unknown,
//! a module binder a binding may pin. The pass keeps a stack of open gatherings: a rule [opens]
//! one where it starts, every read adds to the innermost, and a rule closing hands its frontier to
//! the one enclosing it, since that one read it. A rule run at a [`FactKey`] records its frontier
//! in the shape's residual where it is not empty, and so does a fact written beside it. A memo
//! keeps the frontier it was computed under, and a hit adds it. Only an unknown of a shape on the
//! chain is gathered: a callee's or a nested body's own unknowns are bound in their own frames, so
//! a reader outside reads them as the load does, and a shape leaving the chain takes its own
//! unknowns with it.
//!
//! [opens]: Pass::open

use crate::elaborate::schema_of;
use crate::memory::{BumpVec, collect, resident};
use crate::scope::{
    Address, BodyShape, Caching, Coordinate, FactKey, Frontier, Keyed, Narrowing, PathEntry,
    PathRead, Resolution, Segment, ShapeFacts, Site, Slot, Static, StaticCallable,
    StaticRegistered, StaticSolution, StaticType, Statics, Target, Unknown, UseStatics,
    frontier_at, merge_into,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Side, TypeNode, Variable, bound_above, read_through,
};

use crate::scope::Tree;

use super::{Bound, Level, Pass};

/// Where a lexical variable's home is held, as an unknown's address.
fn address_of(target: Target) -> Address {
    match target {
        Target::Local(slot) => Address::Slot(slot),
        Target::Capture(capture) => Address::Capture(capture),
    }
}

/// The open gatherings of the rules running, innermost last.
pub(super) type Scopes<'p> = BumpVec<'p, BumpVec<'p, Unknown>>;

/// What the pass resolved `entry` to, once it has: read once the walk ends, by the caching
/// fixpoint alone.
pub(super) fn resolution_of<'graph>(entry: &PathEntry<'graph>) -> Option<Resolution<'graph>> {
    entry.resolution()
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
    /// Open a gathering for a rule starting to run.
    pub(super) fn open(&self) {
        self.open_with(&[]);
    }

    /// Open a gathering seeded with `seed`: what the rule is handed was read by its caller.
    pub(super) fn open_with(&self, seed: &[Unknown]) {
        let mut scope = BumpVec::with_capacity_in(seed.len(), self.scratch);
        scope.extend_from_slice(seed);
        self.edit_scopes(|scopes| scopes.push(scope));
    }

    /// Close the innermost gathering, hand what it gathered to the one enclosing it, and lay its
    /// frontier down.
    pub(super) fn close(&self) -> Frontier<'graph> {
        let writer = self.writer;
        self.edit_scopes(|scopes| {
            let mut closed = scopes.pop().expect("a gathering is open");
            if let Some(enclosing) = scopes.last_mut() {
                merge_into(enclosing, &closed);
            }
            Frontier::laid(writer, &mut closed)
        })
    }

    /// [`close`](Self::close) the gathering of the rule run at `key` of the shape at `level`, and
    /// record its frontier in the shape's residual where it waits on an unknown.
    pub(super) fn close_as(&mut self, level: usize, key: FactKey) -> Frontier<'graph> {
        let frontier = self.close();
        self.keep(level, key, frontier);
        frontier
    }

    /// Record `frontier` in the residual of the shape at `level` for the fact at `key`.
    pub(super) fn keep(&mut self, level: usize, key: FactKey, frontier: Frontier<'graph>) {
        if !self.unfilled && !frontier.is_empty() {
            self.chain[level].residual.push((key, frontier));
        }
    }

    /// What the innermost gathering holds so far, laid down.
    pub(super) fn snapshot(&self) -> Frontier<'graph> {
        let writer = self.writer;
        self.edit_scopes(|scopes| match scopes.last() {
            Some(open) => Frontier::from_sorted(collect(writer, open.iter().copied())),
            None => Frontier::EMPTY,
        })
    }

    /// The depth of the gathering stack, which a refusal the pass keeps unwinds to.
    pub(super) fn depth(&self) -> usize {
        self.edit_scopes(|scopes| scopes.len())
    }

    /// Unwind the gathering stack to `depth`: a refused quote's rules closed nothing.
    pub(super) fn unwind(&self, depth: usize) {
        self.edit_scopes(|scopes| scopes.truncate(depth));
    }

    /// Add `unknowns` to the innermost gathering, each of a shape on the chain.
    pub(super) fn note(&self, unknowns: &[Unknown]) {
        if unknowns.is_empty() {
            return;
        }
        self.edit_scopes(|scopes| {
            let Some(open) = scopes.last_mut() else {
                return;
            };
            for unknown in unknowns {
                if self.on_chain(unknown.segment)
                    && let Err(at) = open.binary_search(unknown)
                {
                    open.insert(at, *unknown);
                }
            }
        });
    }

    /// [`note`](Self::note) a frontier.
    pub(super) fn note_frontier(&self, frontier: Frontier<'_>) {
        self.note(frontier.unknowns());
    }

    /// Drop each unknown `shape` declares from the innermost gathering, as `shape` leaves the
    /// chain.
    pub(super) fn leave(&self, shape: &BodyShape<'graph>) {
        let segment = shape.segment();
        self.edit_scopes(|scopes| {
            if let Some(open) = scopes.last_mut() {
                open.retain(|unknown| unknown.segment != segment);
            }
        });
    }

    fn on_chain(&self, segment: Segment) -> bool {
        (self.chain.iter()).any(|level| level.shape.segment() == segment)
    }

    fn edit_scopes<R>(&self, edit: impl FnOnce(&mut Scopes<'p>) -> R) -> R {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let out = edit(&mut scopes);
        self.scopes.set(scopes);
        out
    }

    /// The unknown the lexical variable at `variable`'s level is, read from the shape at
    /// `level`: its home along the chain. `None` for a path variable, whose dependence its
    /// resolution carries.
    pub(super) fn variable_unknown(&self, level: usize, variable: usize) -> Option<Unknown> {
        let mut home = level;
        loop {
            let at = &self.chain[home];
            let homed = self
                .homes(at.shape)
                .iter()
                .find(|(held, _)| *held == variable);
            if let Some((_, target)) = homed {
                return Some(Unknown {
                    segment: at.shape.segment(),
                    at: address_of(*target),
                });
            }
            if at.roots_chain || home == 0 {
                return None;
            }
            home -= 1;
        }
    }

    /// Check, in debug builds, that the innermost gathering holds the unknown of every lexical
    /// variable `values`, read in the shape at `level`, name: a variable no read gathered
    /// reached the fact past the store.
    pub(super) fn sound(&self, level: usize, values: &[Parametric]) {
        if !cfg!(debug_assertions) {
            return;
        }
        let (types, scratch) = (self.types, self.scratch);
        let open = self.edit_scopes(|scopes| scopes.last().cloned());
        let Some(open) = open else {
            return;
        };
        for value in values {
            read_through(types, scratch, *value, Side::Above, &mut |variable| {
                if let Variable::Lexical { level: at, .. } = variable
                    && let Some(unknown) = self.variable_unknown(level, at)
                {
                    assert!(
                        open.binary_search(&unknown).is_ok(),
                        "a fact naming {unknown:?} gathered it"
                    );
                }
                None
            });
        }
    }

    /// The type channel's cell for the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'graph> {
        self.note_frontier(frontier_at(shape.channel_residual(), FactKey::Site(site)));
        shape.typed_expression(site)
    }

    /// The type channel's cell for the type binder at `slot` of `shape`, and the binder itself
    /// where the channel left it unknown.
    pub(super) fn declared_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        self.note_frontier(frontier_at(shape.channel_residual(), FactKey::Slot(slot)));
        let typed = shape.declared_type(slot);
        if shape.declarations(slot).is_some() && !matches!(typed, Static::Closed(_)) {
            self.note(&[Unknown {
                segment: shape.segment(),
                at: Address::Slot(slot),
            }]);
        }
        typed
    }

    /// The type channel's cell for the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'graph> {
        self.note_frontier(frontier_at(shape.channel_residual(), FactKey::Slot(slot)));
        shape.registered_type(slot)
    }

    /// The type channel's cell for the callable body `body`'s type.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'graph> {
        self.note_frontier(frontier_at(body.channel_residual(), FactKey::Shape));
        body.callable_type()
    }

    /// The function type of the callable body `body` as its body reads it, its own group at its
    /// group levels — each of which its own `FOR ALL` names is: `None` where the load did not type
    /// the callable.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>) -> Option<Parametric> {
        let map = match self.callable_cell(body) {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        for (name, _) in map.iter() {
            if let Some((slot, _)) = body.slot(BinderSymbol::Type(name)) {
                self.note(&[Unknown {
                    segment: body.segment(),
                    at: Address::Slot(slot),
                }]);
            }
        }
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
        self.note_frontier(body.shape_facts().born_frontier);
        body.born_instance()
    }

    /// The summary of the callable body `body`, where the pass typed it: `Some(None)` where it has
    /// none.
    pub(super) fn summary_of(&self, body: &BodyShape<'graph>) -> Option<Option<Tree<'graph>>> {
        let facts = body.shape_facts();
        self.note_frontier(facts.summary_frontier);
        facts.summary
    }

    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where the pass typed it
    /// exactly.
    pub(super) fn signature_of(&self, body: &BodyShape<'graph>) -> Option<KType> {
        let facts = body.shape_facts();
        self.note_frontier(facts.signature_frontier);
        facts.signature
    }

    /// The last statement's static type of the quote's code `code` as it runs unfilled.
    pub(super) fn ran_of(&self, code: &BodyShape<'graph>) -> Option<Interval> {
        let facts = code.shape_facts();
        self.note_frontier(facts.ran_frontier);
        facts.ran
    }

    /// The unknown the binder at `slot` of the shape at `level` is, where it is a module binder
    /// whose static type `bound` a binding may pin: its upper end an application of a signature
    /// that leaves a head parameter unpinned.
    fn module_unknown(&self, level: usize, slot: Slot, bound: Bound) -> Option<Unknown> {
        let shape = self.chain[level].shape;
        let (BinderSymbol::Value(_), DeclaredType::Type(typed)) = (shape.slot_name(slot), bound)
        else {
            return None;
        };
        let (types, scratch) = (self.types, self.scratch);
        let upper = bound_above(types, scratch, typed.upper);
        let (signature, pins) = match types.node(upper) {
            TypeNode::Signature { .. } => (upper, None),
            TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
            _ => return None,
        };
        let schema = schema_of(signature, types)?;
        let unpinned = (schema.parameters.iter()).any(|(name, _)| {
            pins.is_none_or(|pins| pins.get(BinderSymbol::Type(*name).symbol()).is_none())
        });
        unpinned.then_some(Unknown {
            segment: shape.segment(),
            at: Address::Slot(slot),
        })
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

    /// The static type of the binder at `slot` of the shape at `level`, as the pass has it now,
    /// and the binder itself where it is a module binder a binding may pin.
    pub(super) fn binder_at(&self, level: usize, slot: Slot) -> Bound {
        let at = &self.chain[level];
        let bound = at.binders[slot.index()];
        self.note_frontier(at.binder_frontiers[slot.index()]);
        if let Some(unknown) = self.module_unknown(level, slot, bound) {
            self.note(&[unknown]);
        }
        bound
    }

    /// The static type of the binder at `slot` of the shape at `level` beside its frontier,
    /// read without gathering it.
    pub(super) fn binder_fact(&self, level: usize, slot: Slot) -> (Bound, Frontier<'graph>) {
        let at = &self.chain[level];
        (at.binders[slot.index()], at.binder_frontiers[slot.index()])
    }

    /// Fix the static type of the binder at `slot` of the shape at `level`, read under
    /// `frontier`.
    pub(super) fn set_binder(
        &mut self,
        level: usize,
        slot: Slot,
        bound: Bound,
        frontier: Frontier<'graph>,
    ) {
        if let DeclaredType::Type(typed) = bound {
            self.sound(level, &[typed.lower, typed.upper]);
        }
        let at = &mut self.chain[level];
        at.binders[slot.index()] = bound;
        at.binder_frontiers[slot.index()] = frontier;
    }

    /// The static type of statement `index` of the shape at `level`, as the pass has it now.
    pub(super) fn statement_at(&self, level: usize, index: usize) -> Interval {
        let at = &self.chain[level];
        self.note_frontier(at.statement_frontiers[index]);
        at.statements[index]
    }

    /// The static type of the last statement of the shape at `level`, where it has one.
    pub(super) fn last_statement(&self, level: usize) -> Option<Interval> {
        let last = self.chain[level].statements.len().checked_sub(1)?;
        Some(self.statement_at(level, last))
    }

    /// Fix the static type of statement `index` of the shape at `level`, read under `frontier`.
    pub(super) fn set_statement(
        &mut self,
        level: usize,
        index: usize,
        typed: Interval,
        frontier: Frontier<'graph>,
    ) {
        self.sound(level, &[typed.lower, typed.upper]);
        let at = &mut self.chain[level];
        at.statements[index] = typed;
        at.statement_frontiers[index] = frontier;
    }

    /// The static type the pass recorded for the part at `site` of the shape at `level`, its
    /// latest where it recorded more than one.
    pub(super) fn part_type(&self, level: usize, site: Site) -> Option<Interval> {
        let at = &self.chain[level];
        let (index, (_, typed)) =
            (at.parts.iter().enumerate().rev()).find(|(_, (held, _))| *held == site)?;
        self.note_frontier(at.part_frontiers[index]);
        Some(*typed)
    }

    /// Record the static type of the part at `site` of the shape at `level`, read under what the
    /// innermost gathering holds.
    pub(super) fn record_part(&mut self, level: usize, site: Site, typed: Interval) {
        self.sound(level, &[typed.lower, typed.upper]);
        let frontier = self.snapshot();
        let at = &mut self.chain[level];
        at.parts.push((site, typed));
        at.part_frontiers.push(frontier);
    }

    /// Record the static type of the part at `site` of the shape at `level` the rule run at its
    /// site typed, and close that run's gathering: its frontier is the part's.
    pub(super) fn close_part(&mut self, level: usize, site: Site, typed: Interval) {
        self.record_part(level, site, typed);
        self.close_as(level, FactKey::Site(site));
    }

    /// Record a part's static type written beside the rule run enclosing it, which has no rule run
    /// of its own, under what the innermost gathering holds.
    pub(super) fn record_written_part(&mut self, level: usize, site: Site, typed: Interval) {
        self.record_part(level, site, typed);
        let frontier = self.snapshot();
        self.keep(level, FactKey::Site(site), frontier);
    }

    /// What the pass fixed about the keyworded use at `index` of the shape at `level`.
    pub(super) fn use_at(&self, level: usize, index: usize) -> UseStatics<'graph> {
        let at = &self.chain[level];
        self.note_frontier(at.use_frontiers[index]);
        at.uses[index]
    }

    /// Fix the narrowing of the keyworded use at `index` of the shape at `level`, under what the
    /// innermost gathering holds.
    pub(super) fn set_narrowing(
        &mut self,
        level: usize,
        index: usize,
        narrowing: Narrowing<'graph>,
    ) {
        let frontier = self.snapshot();
        let at = &mut self.chain[level];
        at.uses[index].narrowing = narrowing;
        at.use_frontiers[index] = frontier;
    }

    /// Fix the contributions of the keyworded use at `index` of the shape at `level`, under what
    /// the innermost gathering holds.
    pub(super) fn set_contributions(
        &mut self,
        level: usize,
        index: usize,
        contributions: &'graph [StaticType<'graph>],
    ) {
        let frontier = self.snapshot();
        let at = &mut self.chain[level];
        at.uses[index].contributions = contributions;
        at.use_frontiers[index] = frontier;
    }

    /// The closed point each variable of the candidate selected at the use at `site` of the shape
    /// at `level` solves to, where the pass selected one there.
    pub(super) fn selection_at(&self, level: usize, site: Site) -> Option<&'graph [Option<KType>]> {
        let at = &self.chain[level];
        if let Some(index) = at.shape.candidate_lists().position(site) {
            self.note_frontier(at.use_frontiers[index]);
        }
        (at.selections.iter())
            .find(|(held, _)| *held == site)
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

    /// Fix the solution the callable body `body` is born instantiated at, under what the
    /// innermost gathering holds.
    pub(super) fn fix_born_instance(
        &self,
        body: &BodyShape<'graph>,
        solution: StaticSolution<'graph>,
    ) {
        let frontier = self.snapshot();
        body.fix_born_instance(solution);
        body.fix_shape_facts(|facts| facts.born_frontier = frontier);
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
            use_frontiers: collect(writer, at.use_frontiers.iter().copied()),
        };
        // A rule run twice at one key, or a fact written beside one, waits on all it read.
        at.residual.sort_by_key(|(key, _)| *key);
        let mut residual: BumpVec<'_, (FactKey, Frontier<'graph>)> =
            BumpVec::with_capacity_in(at.residual.len(), scratch);
        for (key, frontier) in at.residual.iter() {
            match residual.last_mut() {
                Some((held, joined)) if held == key => *joined = joined.union(*frontier, writer),
                _ => residual.push((*key, *frontier)),
            }
        }
        if !residual.is_empty() {
            shape.fix_statics_residual(Keyed::from_sorted(collect(
                writer,
                residual.iter().copied(),
            )));
        }
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

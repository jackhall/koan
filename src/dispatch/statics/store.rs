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
//! In a [resumption](super::resume) the store reads each fact as the resumption has it — the type
//! channel's as its resumption settled it, a shape's own facts as this walk re-ran them, else as
//! the resumptions it extends left them, else the load's — and writes each fact beside the load's.
//! A name the resumption binds is closed, and no gathering holds it.
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

use crate::scope::{ChannelSettled, Residual, StaticsSettled, Tree, in_body_of};

use super::resume::Whole;
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
        if self.unfilled {
            return;
        }
        let writer = self.writer;
        let at = &mut self.chain[level];
        // A resumption's walk starts from the residual its skipped facts leave: a fact it re-runs
        // replaces its entry.
        if self.resume.is_some() && at.kept.insert(key) {
            at.residual.remove(&key);
        }
        // A rule run twice at one key, or a fact written beside one, waits on all it read.
        if !frontier.is_empty() {
            (at.residual.entry(key))
                .and_modify(|held| *held = held.union(frontier, writer))
                .or_insert(frontier);
        }
    }

    /// The frontier the shape at `level` holds for the fact at `key`, as the walk has it now.
    pub(super) fn staged(&self, level: usize, key: FactKey) -> Frontier<'graph> {
        (self.chain[level].residual.get(&key)).map_or(Frontier::EMPTY, |frontier| *frontier)
    }

    /// Whether the rule at `key` of the shape at `level` runs: always at the load, and in a
    /// resumption where the fact's frontier waits on what it binds — once, since a re-run fact
    /// waits on nothing it binds. Counts each fact a resumption re-runs.
    pub(super) fn runs(&self, level: usize, key: FactKey) -> bool {
        let runs = self.reruns(level, key);
        if let Some(resume) = &self.resume
            && runs
            && !self.unfilled
        {
            resume.rerun(self.chain[level].shape.segment(), key);
        }
        runs
    }

    /// [`runs`](Self::runs), counting nothing.
    pub(super) fn reruns(&self, level: usize, key: FactKey) -> bool {
        match &self.resume {
            Some(resume) if !self.unfilled => resume.wakes(self.staged(level, key)),
            _ => true,
        }
    }

    /// The unknown the slot `slot` of the shape at `level` is, where a binding gives it a type.
    pub(super) fn unknown_at(&self, level: usize, slot: Slot) -> Unknown {
        Unknown {
            segment: self.chain[level].shape.segment(),
            at: Address::Slot(slot),
        }
    }

    /// Whether a unit of `shape` types `slot` as a member of a component of value binders.
    pub(super) fn component_types(&self, shape: &BodyShape<'graph>, slot: Slot) -> bool {
        let index = shape.component_index(slot);
        let component = &shape.components()[index.index()];
        shape.declarations(component.members[0]).is_none()
            && (shape.units().iter())
                .any(|unit| unit.work == crate::scope::UnitWork::Component(index))
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
                // A name the resumption binds is closed.
                if self.on_chain(unknown.segment)
                    && self.binds(*unknown).is_none()
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
        self.note_frontier(frontier_at(
            self.channel_residual(shape),
            FactKey::Site(site),
        ));
        let settled = |held: &ChannelSettled<'graph>| held.expressions.get(site).map(|(_, at)| *at);
        (self.channel_cell(shape, settled)).unwrap_or_else(|| shape.typed_expression(site))
    }

    /// The type channel's residual of `shape`, as a resumption leaves it.
    fn channel_residual(&self, shape: &BodyShape<'graph>) -> Residual<'graph> {
        (self.resume.as_ref())
            .and_then(|resume| resume.channel_residual(shape.segment()))
            .unwrap_or_else(|| shape.channel_residual())
    }

    /// The type channel cell of `shape` a resumption settled, read through `of`.
    fn channel_cell<T>(
        &self,
        shape: &BodyShape<'graph>,
        of: impl Fn(&ChannelSettled<'graph>) -> Option<T>,
    ) -> Option<T> {
        self.resume.as_ref()?.channel_cell(shape.segment(), of)
    }

    /// The type a resumption binds `unknown` to.
    pub(super) fn binds(&self, unknown: Unknown) -> Option<KType> {
        self.resume.as_ref()?.bound(unknown)
    }

    /// The type channel's cell for the type binder at `slot` of `shape`, and the binder itself
    /// where the channel left it unknown.
    pub(super) fn declared_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        self.note_frontier(frontier_at(
            self.channel_residual(shape),
            FactKey::Slot(slot),
        ));
        let settled = |held: &ChannelSettled<'graph>| held.declared.get(slot).map(|(_, at)| *at);
        let typed =
            (self.channel_cell(shape, settled)).unwrap_or_else(|| shape.declared_type(slot));
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
        self.note_frontier(frontier_at(
            self.channel_residual(shape),
            FactKey::Slot(slot),
        ));
        let settled = |held: &ChannelSettled<'graph>| held.registered.get(slot).map(|(_, at)| *at);
        (self.channel_cell(shape, settled)).unwrap_or_else(|| shape.registered_type(slot))
    }

    /// The type channel's cell for the callable body `body`'s type.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'graph> {
        self.note_frontier(frontier_at(self.channel_residual(body), FactKey::Shape));
        (self.channel_cell(body, |held| held.callable)).unwrap_or_else(|| body.callable_type())
    }

    /// The function type of the callable body `body` as its body reads it, its own group at its
    /// group levels — each a lexical variable of one of its own `FOR ALL` names, which the read
    /// gathers where the type names it: `None` where the load did not type the callable.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>) -> Option<Parametric> {
        let map = match self.callable_cell(body) {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        let levels =
            (self.channel_cell(body, |held| held.levels)).unwrap_or_else(|| body.group_levels());
        let (types, scratch) = (self.types, self.scratch);
        let ktype = in_body_of(self.callable_cell(body), levels, types, scratch)?;
        let mut named = BumpVec::new_in(scratch);
        read_through(types, scratch, ktype, Side::Above, &mut |variable| {
            if let Variable::Lexical { level, .. } = variable {
                named.push(level);
            }
            None
        });
        for (name, index) in map.iter() {
            let level = levels.get(index).map(|at| types.node(*at));
            let Some(TypeNode::Lexical { level, .. }) = level else {
                continue;
            };
            if named.contains(&level)
                && let Some((slot, _)) = body.slot(BinderSymbol::Type(name))
            {
                self.note(&[Unknown {
                    segment: body.segment(),
                    at: Address::Slot(slot),
                }]);
            }
        }
        Some(ktype)
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
            || self.resume.as_ref().is_some_and(|resume| {
                let segment = code.segment();
                resume.channel_cell(segment, |held| held.refusal).is_some()
                    || resume.own(&resume.refusals, segment).is_some()
                    || resume.over.refusal(segment).is_some()
            })
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
        root.channel_paths()
    }

    /// Every path entry of the chain `root` roots the load minted, the type channel's first.
    pub(super) fn loaded_entries(
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
        let facts = self.facts_of(body, Whole::Born)?;
        self.note_frontier(facts.born_frontier);
        let Some(resume) = &self.resume else {
            return body.born_instance();
        };
        let segment = body.segment();
        (resume.own(&resume.born, segment))
            .or_else(|| resume.over.born(segment))
            .or_else(|| body.born_instance())
    }

    /// The facts the static pass fixed about `body` as a whole, as the walk has them: `None` in a
    /// resumption where `whole` waits on what it binds and the walk has not re-run it yet.
    fn facts_of(&self, body: &BodyShape<'graph>, whole: Whole) -> Option<ShapeFacts<'graph>> {
        let facts = self.shape_facts(body);
        let Some(resume) = &self.resume else {
            return Some(facts);
        };
        let frontier = match whole {
            Whole::Summary => facts.summary_frontier,
            Whole::Signature => facts.signature_frontier,
            Whole::Ran => facts.ran_frontier,
            Whole::Born => facts.born_frontier,
        };
        let ready = !resume.wakes(frontier) || resume.reran_whole(body.segment(), whole);
        ready.then_some(facts)
    }

    /// `body`'s facts as a whole as the walk has them, whether or not they wait on it.
    fn shape_facts(&self, body: &BodyShape<'graph>) -> ShapeFacts<'graph> {
        let Some(resume) = &self.resume else {
            return body.shape_facts();
        };
        let segment = body.segment();
        (resume.own(&resume.facts, segment))
            .or_else(|| resume.over.facts(segment))
            .unwrap_or_else(|| body.shape_facts())
    }

    /// Whether `body` is recursive: met while it was typed at the load, or since.
    pub(super) fn recursive_at_load(&self, body: &BodyShape<'graph>) -> bool {
        self.resume.is_some() && body.shape_facts().recursive
    }

    /// Whether a resumption re-runs `body`'s fact `whole`.
    pub(super) fn runs_whole(&self, body: &BodyShape<'graph>, whole: Whole) -> bool {
        self.resume.is_none() || self.unfilled || self.facts_of(body, whole).is_none()
    }

    /// The summary of the callable body `body`, where the pass typed it: `Some(None)` where it has
    /// none.
    pub(super) fn summary_of(&self, body: &BodyShape<'graph>) -> Option<Option<Tree<'graph>>> {
        let facts = self.facts_of(body, Whole::Summary)?;
        self.note_frontier(facts.summary_frontier);
        facts.summary
    }

    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where the pass typed it
    /// exactly.
    pub(super) fn signature_of(&self, body: &BodyShape<'graph>) -> Option<KType> {
        let facts = self.facts_of(body, Whole::Signature)?;
        self.note_frontier(facts.signature_frontier);
        facts.signature
    }

    /// The last statement's static type of the quote's code `code` as it runs unfilled.
    pub(super) fn ran_of(&self, code: &BodyShape<'graph>) -> Option<Interval> {
        let facts = self.facts_of(code, Whole::Ran)?;
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

    /// Gather the unknown the binder at `slot` of the shape at `level`, of static type `bound`,
    /// is itself, where it is one: a type name the chain numbers, or a module binder. A binding of
    /// it changes its fact though nothing it read did.
    pub(super) fn note_own(&self, level: usize, slot: Slot, bound: Bound) {
        let shape = self.chain[level].shape;
        let homed = (self.homes(shape).iter()).any(|(_, target)| *target == Target::Local(slot));
        let own = match homed {
            true => Some(self.unknown_at(level, slot)),
            false => self.module_unknown(level, slot, bound),
        };
        if let Some(own) = own {
            self.note(&[own]);
        }
    }

    /// Whether the pass has fixed `shape`'s cell.
    pub(super) fn typed_shape(&self, shape: &BodyShape<'graph>) -> bool {
        let Some(resume) = &self.resume else {
            return shape.statics().is_some();
        };
        // A shape the resumption skips is typed as it was.
        let segment = shape.segment();
        resume.own(&resume.statics, segment).is_some()
            || (!resume.wakes(shape.reach()) && self.statics_of(shape).is_some())
    }

    /// `shape`'s cell and residual as the resumptions this one extends left them, else as the
    /// load did; `None` where neither typed it.
    pub(super) fn statics_of(&self, shape: &BodyShape<'graph>) -> Option<StaticsSettled<'graph>> {
        let settled =
            (self.resume.as_ref()).and_then(|resume| resume.over.statics(shape.segment()));
        settled.or_else(|| {
            Some(StaticsSettled {
                statics: shape.statics()?,
                residual: shape.statics_residual(),
            })
        })
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

    /// Put back the static type of the binder at `slot` of the shape at `level`, a fact a
    /// resumption skipped, beside the frontier it was read under.
    pub(super) fn restore_binder(
        &mut self,
        level: usize,
        slot: Slot,
        bound: Bound,
        frontier: Frontier<'graph>,
    ) {
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
        at.selections.get(&site).copied()
    }

    /// Record the points the candidate selected at the use at `site` solves to.
    pub(super) fn record_selection(
        &mut self,
        level: usize,
        site: Site,
        points: &'graph [Option<KType>],
    ) {
        self.chain[level].selections.insert(site, points);
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
        self.keep_side(level, site);
        self.chain[level].trees.push((site, tree));
    }

    /// Record that the ascription or annotated binder's type part at `site` is settled.
    pub(super) fn record_settled(&mut self, level: usize, site: Site) {
        self.keep_side(level, site);
        self.chain[level].settled.push(site);
    }

    /// Record what the pass fixed where a quantified function is read at `site`.
    pub(super) fn record_instance_read(
        &mut self,
        level: usize,
        site: Site,
        read: crate::scope::InstanceRead<'graph>,
    ) {
        self.keep_side(level, site);
        self.chain[level].instances.push((site, read));
    }

    /// Record a call by name's contributions by its argument part's site.
    pub(super) fn record_named(
        &mut self,
        level: usize,
        site: Site,
        contributions: &'graph [StaticType<'graph>],
    ) {
        self.keep_side(level, site);
        self.chain[level].named.push((site, contributions));
    }

    /// Keep what the innermost gathering holds as the frontier of a side fact recorded at `site`:
    /// the rule recording it records it again exactly where nothing it read so far changes.
    fn keep_side(&mut self, level: usize, site: Site) {
        let frontier = self.snapshot();
        self.keep(level, FactKey::Side(site), frontier);
    }

    /// Fix the solution the callable body `body` is born instantiated at, under what the
    /// innermost gathering holds.
    pub(super) fn fix_born_instance(
        &self,
        body: &BodyShape<'graph>,
        solution: StaticSolution<'graph>,
    ) {
        let frontier = self.snapshot();
        if let Some(resume) = &self.resume {
            resume.write(&resume.born, body.segment(), solution);
        } else {
            body.fix_born_instance(solution);
        }
        self.fix_shape_facts(body, Whole::Born, |facts| facts.born_frontier = frontier);
    }

    /// Start a resumption's walk of the shape at `level` from every fact it skips — each whose
    /// frontier waits on nothing it binds — as the resumptions it extends left it, else as the load
    /// did, beside the shape's residual.
    pub(super) fn prefill(&mut self, level: usize) {
        let Some(resume) = &self.resume else {
            return;
        };
        let at = &self.chain[level];
        if self.unfilled || resume.over.everything {
            return;
        }
        let Some(held) = self.statics_of(at.shape) else {
            return;
        };
        let (statics, residual) = (held.statics, held.residual);
        let skipped = |key| {
            let frontier = frontier_at(residual, key);
            (!resume.wakes(frontier)).then_some(frontier)
        };
        let shape = at.shape;
        let side = |site| skipped(FactKey::Side(site)).is_some();
        let at = &mut self.chain[level];
        // A side fact the walk does not start from is recorded again or not at all.
        at.residual
            .extend((residual.iter().copied()).filter(|(key, _)| match key {
                FactKey::Side(site) => side(*site),
                _ => true,
            }));
        for (index, bound) in statics.binders.iter().enumerate() {
            if let Some(frontier) = skipped(FactKey::Slot(Slot(index as u32))) {
                at.binders[index] = *bound;
                at.binder_frontiers[index] = frontier;
            }
        }
        for (index, typed) in statics.statements.iter().enumerate() {
            if let Some(frontier) = skipped(FactKey::Statement(index as u32)) {
                at.statements[index] = *typed;
                at.statement_frontiers[index] = frontier;
            }
        }
        for (site, typed) in statics.parts.iter() {
            if let Some(frontier) = skipped(FactKey::Site(*site)) {
                at.parts.push((*site, *typed));
                at.part_frontiers.push(frontier);
            }
        }
        let lists = shape.candidate_lists();
        let use_skipped = |site: Site| {
            let index = lists.position(site)?;
            let frontier = *statics.use_frontiers.get(index)?;
            (!resume.wakes(frontier)).then_some((index, frontier))
        };
        for (site, fixed) in statics.uses.iter() {
            if let Some((index, frontier)) = use_skipped(*site) {
                at.uses[index] = *fixed;
                at.use_frontiers[index] = frontier;
            }
        }
        (at.selections).extend(
            statics
                .selections
                .iter()
                .filter(|(site, _)| use_skipped(*site).is_some()),
        );
        at.settled
            .extend(statics.settled.iter().filter(|site| side(**site)));
        at.instances
            .extend(statics.instances.iter().filter(|(site, _)| side(*site)));
        at.named
            .extend(statics.named.iter().filter(|(site, _)| side(*site)));
        at.trees
            .extend(statics.trees.iter().filter(|(site, _)| side(*site)));
        at.throughs.extend(statics.throughs.iter());
    }

    /// Lay the type captures the pass added to `shape` down.
    pub(super) fn fix_type_captures(
        &self,
        shape: &BodyShape<'graph>,
        captures: &'graph [Coordinate],
    ) {
        if self.resume.is_none() {
            shape.fix_type_captures(captures);
        }
    }

    /// Lay what the pass typed of the shape at `level` down in its cell.
    pub(super) fn fix(&mut self, level: usize) {
        if self.unfilled {
            return;
        }
        let (writer, scratch) = (self.writer, self.scratch);
        // A resumption keeps the load's bookkeeping for the run.
        let loaded = match self.resume {
            Some(_) => self
                .statics_of(self.chain[level].shape)
                .map(|held| held.statics),
            None => None,
        };
        let class = match loaded {
            Some(loaded) => loaded.class,
            None => self.class(level),
        };
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
        if self.resume.is_some() {
            newest(&mut at.instances);
            newest(&mut at.named);
            newest(&mut at.trees);
            at.settled.sort_unstable();
            at.settled.dedup();
        }
        let mut selections = BumpVec::with_capacity_in(at.selections.len(), scratch);
        selections.extend(at.selections.iter().map(|(site, points)| (*site, *points)));
        let shape = at.shape;
        let statics = Statics {
            parts: Keyed::sorted(writer, &mut at.parts),
            statements: collect(writer, at.statements.iter().copied()),
            binders: collect(writer, at.binders.iter().copied()),
            uses: Keyed::sorted(writer, &mut uses),
            selections: Keyed::sorted(writer, &mut selections),
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
        let statics = match loaded {
            Some(loaded) => Statics {
                minting: loaded.minting,
                digested: loaded.digested,
                view_slots: loaded.view_slots,
                handoffs: loaded.handoffs,
                ..statics
            },
            None => statics,
        };
        let mut residual: BumpVec<'_, (FactKey, Frontier<'graph>)> =
            BumpVec::with_capacity_in(at.residual.len(), scratch);
        residual.extend(at.residual.iter().map(|(key, frontier)| (*key, *frontier)));
        let residual = Keyed::sorted(writer, &mut residual);
        if let Some(resume) = &self.resume {
            let settled = StaticsSettled { statics, residual };
            resume.write(&resume.statics, shape.segment(), settled);
            return;
        }
        if !residual.is_empty() {
            shape.fix_statics_residual(residual);
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
        whole: Whole,
        edit: impl FnOnce(&mut ShapeFacts<'graph>),
    ) {
        let Some(resume) = &self.resume else {
            debug_assert!(
                !whole.is_set(&shape.shape_facts()),
                "a shape's fact as a whole is fixed once"
            );
            return shape.fix_shape_facts(edit);
        };
        debug_assert!(
            resume.over.everything || !resume.reran_whole(shape.segment(), whole),
            "a shape's fact as a whole re-runs at most once in a resumption"
        );
        let mut facts = self.shape_facts(shape);
        edit(&mut facts);
        resume.write(&resume.facts, shape.segment(), facts);
        resume.rerun_whole(shape.segment(), whole);
    }

    /// Keep the refusal of the quote's code `code`.
    pub(super) fn refuse_code(
        &self,
        code: &BodyShape<'graph>,
        refusal: &'graph crate::scope::ShapeError<'graph>,
    ) {
        match &self.resume {
            Some(resume) => resume.write(&resume.refusals, code.segment(), refusal),
            None => code.refuse_typing(refusal),
        }
    }

    /// Lay where `holder`'s types intern at run down.
    pub(super) fn fix_caching(&self, holder: &BodyShape<'graph>, caching: Caching<'graph>) {
        holder.fix_caching(resident(self.writer, caching));
    }

    /// Fix what the path `entry` resolved to.
    pub(super) fn fix_resolution(
        &self,
        root: &BodyShape<'graph>,
        entry: &PathEntry<'graph>,
        resolution: Resolution<'graph>,
    ) {
        match &self.resume {
            Some(resume) => {
                let key = (root.segment(), entry.level);
                resume.write(&resume.resolutions, key, resolution);
            }
            None => entry.fix_resolution(resolution),
        }
    }

    /// What `entry`, a path of the chain `root` roots, resolved to before this walk: as the
    /// resumptions this one extends left it, else as the load did.
    pub(super) fn resolved_before(
        &self,
        root: &BodyShape<'graph>,
        entry: &PathEntry<'graph>,
    ) -> Option<Resolution<'graph>> {
        (self.resume.as_ref())
            .and_then(|resume| resume.over.resolution(root.segment(), entry.level))
            .or_else(|| entry.resolution())
    }

    /// Whether a resumption re-resolves a path whose resolution waits on `frontier`.
    pub(super) fn resolves_again(&self, frontier: Frontier<'_>) -> bool {
        (self.resume.as_ref()).is_some_and(|resume| resume.wakes(frontier))
    }

    /// Lay every path entry of the chain `root` roots down, the type channel's first.
    pub(super) fn fix_paths(
        &self,
        root: &BodyShape<'graph>,
        paths: &'graph [&'graph PathEntry<'graph>],
    ) {
        match self.resume {
            Some(_) => assert_eq!(
                paths.len(),
                root.paths().len(),
                "a resumption mints no path entry"
            ),
            None => root.fix_paths(paths),
        }
    }
}

/// Keep the newest of `held`'s records at each site: a resumption's walk records a site it
/// re-runs after the one it started from.
fn newest<T>(held: &mut BumpVec<'_, (Site, T)>) {
    // A stable sort keeps the records at one site in order, the newest last.
    held.sort_by_key(|(site, _)| *site);
    held.reverse();
    held.dedup_by_key(|(site, _)| *site);
}

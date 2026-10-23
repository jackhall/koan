//! The walk's **façade**: the one door every read and write of a fact goes through, so a rule
//! never names where a fact rests ([the store](../../build/README.md#the-store)).
//!
//! A fact the walk fixed rests in its shape's level while the shape is on the chain, beside the
//! unit that fixed it; an inline body's facts rest in the record of the unit that ran it once the
//! body leaves the chain; a callable's or a quote's code's birth facts in its birth unit's; a
//! body's closing facts in its closing record. A read gathers what the fact waits on, at unit
//! grain: nothing for a fact of the unit running, the gathering so far of a unit still running
//! around it, and a read of the frontier of one that ended, cut to the chain. Only an unknown of a
//! shape on the chain is gathered, and one the binding binds is closed.

use crate::elaborate::schema_of;
use crate::memory::BumpVec;
use crate::scope::{
    Address, BodyShape, Born, Cut, Frontier, InstanceRead, Narrowing, Segment, ShapeKind, Site,
    Slot, Static, StaticCallable, StaticRegistered, StaticSolution, StaticType, Store, Tree, Typed,
    UnitFacts, UnitId, Unknown, UseStatics, in_body_of,
};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Side, TypeNode, Variable};
use crate::type_lattice::{bound_above, read_through};

use super::{Bound, Pass, Tag, Wrote};

/// Which part of a callable's function type a read takes: what the read gathers is the names
/// that part names.
#[derive(Clone, Copy)]
pub(super) enum Part {
    Return,
    Parameter(Symbol),
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Gather what a fact the unit `tag` fixed waits on: nothing where `tag` is the unit running,
    /// a running unit's gathering so far, a read of an ended one's frontier.
    pub(super) fn gather(&self, tag: Tag) {
        if self.open.last() == Some(&tag) {
            return;
        }
        let state = &self.units[tag.0 as usize];
        if let Some(frontier) = state.frontier {
            return self.note_frontier(frontier, state.id.segment);
        }
        let Some(at) = self.open.iter().position(|held| *held == tag) else {
            return;
        };
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let innermost = scopes.len() - 1;
        debug_assert!(at < innermost, "the innermost gathers nothing of itself");
        let (outer, inner) = scopes.split_at_mut(innermost);
        let (outer, inner) = (&outer[at], &mut inner[0]);
        #[cfg(test)]
        let before = inner.unknowns.len() + inner.reads.len();
        (inner.unknowns).extend(
            (outer.unknowns.iter())
                .copied()
                .filter(|unknown| self.gathers(*unknown)),
        );
        // A read kept what lies at or above its cut; of that, what is still on the chain lies at
        // or above the innermost body on the chain enclosing the cut.
        inner.reads.extend(outer.reads.iter().map(|cut| Cut {
            frontier: cut.frontier,
            from: self.meet(cut.from),
        }));
        #[cfg(test)]
        super::count(|counts| counts.noted += inner.unknowns.len() + inner.reads.len() - before);
        self.scopes.set(scopes);
    }

    /// Whether `unknown` is gathered: of a shape on the chain, and left unbound by the binding.
    fn gathers(&self, unknown: Unknown) -> bool {
        (self.chain.iter()).any(|level| level.shape.segment() == unknown.segment)
            && self.store.binding(unknown).is_none()
    }

    /// The innermost body on the chain that is `segment` or encloses it, so a cut there lies on the
    /// chain of `segment`'s record. The chain is a root path: level 0 is the program, which
    /// encloses every body, and each level the body enclosing the next, the levels above a callee's
    /// parent set aside while it is walked on demand.
    fn meet(&self, mut segment: Segment) -> Segment {
        while self.level_of(segment).is_none() {
            segment = (self.shapes.parent(segment))
                .expect("the program encloses every body")
                .0;
        }
        segment
    }

    /// Add `unknowns` to the innermost gathering, unsorted: its frontier sorts and dedups it once,
    /// at `end`.
    pub(super) fn note(&self, unknowns: &[Unknown]) {
        if unknowns.is_empty() {
            return;
        }
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            let gathered = unknowns
                .iter()
                .copied()
                .filter(|unknown| self.gathers(*unknown));
            #[cfg(test)]
            let before = open.unknowns.len();
            open.unknowns.extend(gathered);
            #[cfg(test)]
            super::count(|counts| counts.noted += open.unknowns.len() - before);
        }
        self.scopes.set(scopes);
    }

    /// Add a read of `frontier`, the ended record of a unit or the closing of `segment`, to the
    /// innermost gathering, cut to the chain: one entry however many unknowns it holds. The read
    /// keeps any of its unknowns the binding binds — a base record read before the walk re-runs it
    /// names them — since a frontier is read under the reader's binding, which extends the binding
    /// of the layer that laid it: a layered store wakes only on what its base leaves unbound.
    pub(super) fn note_frontier(&self, frontier: Frontier<'o>, segment: Segment) {
        if frontier.is_empty() {
            return;
        }
        let cut = Cut {
            frontier,
            from: self.meet(segment),
        };
        if !cut.keeps_any() {
            return;
        }
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            open.reads.push(cut);
            #[cfg(test)]
            super::count(|counts| counts.noted += 1);
        }
        self.scopes.set(scopes);
    }

    /// Drop each unknown `shape` declares from the innermost gathering, as `shape` leaves the
    /// chain: a read cut at or inside `shape` is cut at its encloser instead.
    pub(super) fn leave(&self, shape: &BodyShape<'graph>) {
        let segment = shape.segment();
        let parent = self.shapes.parent(segment).map(|(parent, _)| parent);
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            open.unknowns.retain(|unknown| unknown.segment != segment);
            // Only a cut inside `shape`: one at a body set aside beside it, while a callee is
            // walked on demand, may hold a lower segment without lying inside it.
            open.reads
                .retain_mut(|cut| match self.shapes.encloses(segment, cut.from) {
                    false => true,
                    true => parent.inspect(|parent| cut.from = *parent).is_some(),
                });
        }
        self.scopes.set(scopes);
    }

    /// What the binding binds `unknown` to, where it binds it.
    pub(super) fn bound_to(&self, unknown: Unknown) -> Option<crate::scope::Bound<'o>> {
        self.store.binding(unknown)
    }

    /// The closed type the binding gives `unknown`: a type name's type, or a module binder's
    /// carried type.
    pub(super) fn binds(&self, unknown: Unknown) -> Option<KType> {
        match self.bound_to(unknown)? {
            crate::scope::Bound::Type(bound) => Some(bound),
            crate::scope::Bound::Module { carried, .. } => Some(carried),
        }
    }

    /// The unknown the slot `slot` of the shape at `level` is.
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
        // A component's unit, where it has one, is the unit its members bind at.
        let unit = shape.unit_of_slot(component.members[0]).run_index();
        shape.declarations(component.members[0]).is_none()
            && unit.is_some_and(|unit| {
                shape.units()[unit].work == crate::scope::UnitWork::Component(index)
            })
    }

    /// The chain level `segment` sits at, where it is on the chain.
    pub(super) fn level_of(&self, segment: Segment) -> Option<usize> {
        (self.chain.iter()).rposition(|level| level.shape.segment() == segment)
    }

    /// The record of `unit` as the walk has it, its frontier gathered: `None` where it is not
    /// written.
    fn record_of(&self, unit: UnitId) -> Option<&'o UnitFacts<'o>> {
        let (facts, frontier) = self.store.fact(unit)?;
        self.note_frontier(frontier, unit.segment);
        Some(facts)
    }

    /// The state of `unit` in this walk, where it ran or absorbed it.
    fn state_of(&self, unit: UnitId) -> Option<(Tag, &super::UnitState<'p, 'o>)> {
        let tag = self.started.get(unit)?;
        Some((tag, &self.units[tag.0 as usize]))
    }

    /// The birth facts of `body`, a callable or a quote's code, its unit's frontier gathered.
    pub(super) fn born(&self, body: Segment) -> Option<Born<'o>> {
        let unit = self.shapes.birth_unit(body)?;
        if let Some((tag, state)) = self.state_of(unit)
            && let Some((_, born)) = state.born.iter().find(|(held, _)| *held == body)
        {
            self.gather(tag);
            return Some(*born);
        }
        let facts = self.record_of(unit)?;
        facts.born.get(body).map(|(_, born)| *born)
    }

    /// Read `read` off the facts of `segment`, an inline body off the chain, as the unit that ran
    /// it keeps them, its frontier gathered.
    pub(super) fn inline_facts<R>(
        &self,
        segment: Segment,
        read: impl FnOnce(&Typed<'o>) -> R,
    ) -> Option<R> {
        let unit = self.shapes.birth_unit(segment)?;
        if let Some((tag, state)) = self.state_of(unit)
            && let Some((_, typed)) = state.typed.iter().rev().find(|(held, _)| *held == segment)
        {
            self.gather(tag);
            return Some(read(typed));
        }
        let facts = self.record_of(unit)?;
        facts.typed.get(segment).map(|(_, typed)| read(typed))
    }

    /// Read `read` off the facts of `segment`, an owning body off the chain, that `unit` of it
    /// fixed.
    fn owner_facts<R>(
        &self,
        segment: Segment,
        unit: UnitId,
        read: impl FnOnce(&Typed<'o>) -> R,
    ) -> Option<R> {
        let facts = self.record_of(unit)?;
        facts.typed.get(segment).map(|(_, typed)| read(typed))
    }

    /// Read `read` off the facts of `segment` off the chain that hold a fact at `slot`.
    fn off_chain_slot<R>(
        &self,
        segment: Segment,
        slot: Slot,
        read: impl FnOnce(&Typed<'o>) -> R,
    ) -> Option<R> {
        match self.shape_at(segment).kind() {
            ShapeKind::Block | ShapeKind::Module => self.inline_facts(segment, read),
            _ => self.owner_facts(segment, self.shapes.slot_unit(segment, slot), read),
        }
    }

    /// The static type of the binder at `slot` of the shape at `level`, as the walk has it now,
    /// and the binder itself where it is a module binder a binding may pin.
    pub(super) fn binder_at(&self, level: usize, slot: Slot) -> Bound {
        let (bound, tag) = self.chain[level].binders[slot.index()];
        if let Some(tag) = tag {
            self.gather(tag);
        }
        if let Some(unknown) = self.module_unknown(level, slot, bound) {
            self.note(&[unknown]);
        }
        bound
    }

    /// Log that the unit `tag` wrote at `wrote` in the shape at `level`, where that shape is the
    /// unit's own owning body: its record lays what it wrote. An inline body lays itself whole.
    fn wrote(&mut self, level: usize, tag: Tag, wrote: Wrote) {
        let segment = self.chain[level].shape.segment();
        let state = &mut self.units[tag.0 as usize];
        if state.id.segment == segment {
            state.wrote.push(wrote);
        }
    }

    /// The static type of the binder at `slot` of the shape at `level`, read without gathering it.
    pub(super) fn binder_fact(&self, level: usize, slot: Slot) -> Bound {
        self.chain[level].binders[slot.index()].0
    }

    /// Fix the static type of the binder at `slot` of the shape at `level`.
    pub(super) fn set_binder(&mut self, level: usize, slot: Slot, bound: Bound) {
        let tag = self.open.last().copied();
        self.chain[level].binders[slot.index()] = (bound, tag);
        if let Some(tag) = tag {
            self.wrote(level, tag, Wrote::Binder(slot));
        }
    }

    /// The static type of statement `index` of the shape at `level`, as the walk has it now.
    pub(super) fn statement_at(&self, level: usize, index: usize) -> Interval {
        match self.chain[level].statements[index] {
            Some((typed, tag)) => {
                self.gather(tag);
                typed
            }
            None => crate::values::unknown(),
        }
    }

    /// The static type of the last statement of the shape at `level`, where it has one.
    pub(super) fn last_statement(&self, level: usize) -> Option<Interval> {
        let last = self.chain[level].statements.len().checked_sub(1)?;
        Some(self.statement_at(level, last))
    }

    /// Fix the static type of statement `index` of the shape at `level`.
    pub(super) fn set_statement(&mut self, level: usize, index: usize, typed: Interval) {
        let tag = self.innermost();
        self.chain[level].statements[index] = Some((typed, tag));
        self.wrote(level, tag, Wrote::Statement(index));
    }

    /// The static type the walk recorded for the part at `site` of the shape at `level`, its
    /// latest where it recorded more than one.
    pub(super) fn part_type(&self, level: usize, site: Site) -> Option<Interval> {
        let (typed, tag) = self.chain[level].parts.get(&site)?;
        self.gather(*tag);
        Some(*typed)
    }

    /// Record the static type of the part at `site` of the shape at `level`.
    pub(super) fn record_part(&mut self, level: usize, site: Site, typed: Interval) {
        let tag = self.innermost();
        self.chain[level].parts.insert(site, (typed, tag));
        self.wrote(level, tag, Wrote::Part(site));
    }

    /// What the walk fixed about the keyworded use at `index` of the shape at `level`.
    pub(super) fn use_at(&self, level: usize, index: usize) -> UseStatics<'o> {
        let (fixed, tag) = self.chain[level].uses[index];
        if let Some(tag) = tag {
            self.gather(tag);
        }
        fixed
    }

    /// Fix the narrowing of the keyworded use at `index` of the shape at `level`.
    pub(super) fn set_narrowing(&mut self, level: usize, index: usize, narrowing: Narrowing<'o>) {
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.narrowing = narrowing;
        at.1 = Some(tag);
        self.wrote(level, tag, Wrote::Use(index));
    }

    /// Fix the contributions of the keyworded use at `index` of the shape at `level`.
    pub(super) fn set_contributions(
        &mut self,
        level: usize,
        index: usize,
        contributions: &'o [StaticType<'o>],
    ) {
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.contributions = contributions;
        at.1 = Some(tag);
        self.wrote(level, tag, Wrote::Use(index));
    }

    /// The closed point each variable of the candidate selected at the use at `site` of the shape
    /// at `level` solves to, where the walk selected one there.
    pub(super) fn selection_at(&self, level: usize, site: Site) -> Option<&'o [Option<KType>]> {
        let at = &self.chain[level];
        if let Some(index) = at.shape.candidate_lists().position(site)
            && let (_, Some(tag)) = at.uses[index]
        {
            self.gather(tag);
        }
        at.selections.get(&site).map(|(points, _)| *points)
    }

    /// Record the points the candidate selected at the use at `site` solves to.
    pub(super) fn record_selection(
        &mut self,
        level: usize,
        site: Site,
        points: &'o [Option<KType>],
    ) {
        let tag = self.innermost();
        self.chain[level].selections.insert(site, (points, tag));
        self.wrote(level, tag, Wrote::Selection(site));
    }

    /// Whether the shape at `level` locates the lexical variables of the path at `path_level`'s
    /// application already.
    pub(super) fn has_through(&self, level: usize, path_level: usize) -> bool {
        self.chain[level].throughs.contains_key(&path_level)
    }

    /// Record where the shape at `level` reads each lexical variable of the path at
    /// `path_level`'s application.
    pub(super) fn record_through(
        &mut self,
        level: usize,
        path_level: usize,
        located: &'o [crate::scope::Variable],
    ) {
        let tag = self.innermost();
        self.chain[level]
            .throughs
            .entry(path_level)
            .or_insert((located, tag));
        self.wrote(level, tag, Wrote::Through(path_level));
    }

    /// Record the content tree of the view the `:|` at `site` builds.
    pub(super) fn record_view_tree(&mut self, level: usize, site: Site, tree: &'o Tree<'o>) {
        let tag = self.innermost();
        let index = self.chain[level].trees.len();
        self.chain[level].trees.push((site, tree, tag));
        self.wrote(level, tag, Wrote::Tree(index));
    }

    /// Record the content tree of the module binder at `slot` of the shape at `level`.
    pub(super) fn record_content(&mut self, level: usize, slot: Slot, tree: Tree<'o>) {
        let tag = self.innermost();
        let index = self.chain[level].contents.len();
        self.chain[level].contents.push((slot, tree, tag));
        self.wrote(level, tag, Wrote::Content(index));
    }

    /// Record that the ascription or annotated binder's type part at `site` is settled.
    pub(super) fn record_settled(&mut self, level: usize, site: Site) {
        let tag = self.innermost();
        let index = self.chain[level].settled.len();
        self.chain[level].settled.push((site, tag));
        self.wrote(level, tag, Wrote::Settled(index));
    }

    /// Record a minting site of the shape at `level`.
    pub(super) fn record_minting(&mut self, level: usize, site: Site) {
        let tag = self.innermost();
        let index = self.chain[level].minting.len();
        self.chain[level].minting.push((site, tag));
        self.wrote(level, tag, Wrote::Minting(index));
    }

    /// Record what the walk fixed where a quantified function is read at `site`.
    pub(super) fn record_instance_read(
        &mut self,
        level: usize,
        site: Site,
        read: InstanceRead<'o>,
    ) {
        let tag = self.innermost();
        let index = self.chain[level].instances.len();
        self.chain[level].instances.push((site, read, tag));
        self.wrote(level, tag, Wrote::Instance(index));
    }

    /// Record a call by name's contributions by its argument part's site.
    pub(super) fn record_named(
        &mut self,
        level: usize,
        site: Site,
        contributions: &'o [StaticType<'o>],
    ) {
        let tag = self.innermost();
        let index = self.chain[level].named.len();
        self.chain[level].named.push((site, contributions, tag));
        self.wrote(level, tag, Wrote::Named(index));
    }

    /// The type the walk fixed for the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'o> {
        let segment = shape.segment();
        if let Some(level) = self.level_of(segment) {
            return match self.chain[level].expressions.get(&site) {
                Some((typed, tag)) => {
                    self.gather(*tag);
                    *typed
                }
                None => Static::Unknown,
            };
        }
        let expressions = shape.type_expressions();
        let statement = (expressions.binary_search_by_key(&site, |expression| expression.site))
            .ok()
            .map(|at| expressions[at].statement);
        let read = |facts: &Typed<'o>| facts.expressions.get(site).map(|(_, typed)| *typed);
        let typed = match shape.kind() {
            ShapeKind::Block | ShapeKind::Module => self.inline_facts(segment, read),
            _ => statement.and_then(|statement| {
                self.owner_facts(segment, self.shapes.running(segment, statement), read)
            }),
        };
        typed.flatten().unwrap_or(Static::Unknown)
    }

    /// Fix the type of the type expression at `site` of the shape at `level`.
    pub(super) fn set_expression(&mut self, level: usize, site: Site, typed: StaticType<'o>) {
        let tag = self.innermost();
        self.chain[level].expressions.insert(site, (typed, tag));
        self.wrote(level, tag, Wrote::Expression(site));
    }

    /// The declaration the walk fixed for the type binder at `slot` of `shape`, and the binder
    /// itself where it is not closed.
    pub(super) fn declared_cell(&self, shape: &BodyShape<'graph>, slot: Slot) -> StaticType<'o> {
        let segment = shape.segment();
        let typed = match self.level_of(segment) {
            Some(level) => match self.chain[level].declared[slot.index()] {
                Some((typed, tag)) => {
                    self.gather(tag);
                    typed
                }
                None => Static::Unknown,
            },
            None => (self.off_chain_slot(segment, slot, |facts| {
                facts.declared.get(slot).map(|(_, typed)| *typed)
            }))
            .flatten()
            .unwrap_or(Static::Unknown),
        };
        if shape.declarations(slot).is_some() && !matches!(typed, Static::Closed(_)) {
            self.note(&[Unknown {
                segment,
                at: Address::Slot(slot),
            }]);
        }
        typed
    }

    /// Fix the declaration of the type binder at `slot` of the shape at `level`.
    pub(super) fn set_declared(&mut self, level: usize, slot: Slot, typed: StaticType<'o>) {
        let tag = self.innermost();
        self.chain[level].declared[slot.index()] = Some((typed, tag));
        self.wrote(level, tag, Wrote::Declared(slot));
    }

    /// The entry the walk fixed for the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'o> {
        let segment = shape.segment();
        match self.level_of(segment) {
            Some(level) => match self.chain[level].registered[slot.index()] {
                Some((typed, tag)) => {
                    self.gather(tag);
                    typed
                }
                None => Static::Unknown,
            },
            None => (self.off_chain_slot(segment, slot, |facts| {
                facts.registered.get(slot).map(|(_, typed)| *typed)
            }))
            .flatten()
            .unwrap_or(Static::Unknown),
        }
    }

    /// Fix the entry of the registration at `slot` of the shape at `level`.
    pub(super) fn set_registered(&mut self, level: usize, slot: Slot, typed: StaticRegistered<'o>) {
        let tag = self.innermost();
        self.chain[level].registered[slot.index()] = Some((typed, tag));
        self.wrote(level, tag, Wrote::Registered(slot));
    }

    /// The type the walk fixed for the callable body `body`.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'o> {
        self.born(body.segment())
            .map_or(Static::Unknown, |born| born.callable)
    }

    /// Record the birth facts of `body`, a callable the unit running births.
    pub(super) fn set_born(&mut self, body: Segment, born: Born<'o>) {
        let tag = self.innermost();
        let state = &mut self.units[tag.0 as usize];
        match (state.born.iter_mut()).find(|(held, _)| *held == body) {
            Some((_, held)) => *held = born,
            None => state.born.push((body, born)),
        }
    }

    /// The `part` of the function type of the callable body `body` as its body reads it, its own
    /// group at its group levels — each a lexical variable of one of its own `FOR ALL` names, which
    /// the read gathers where the part names it: `None` where the walk did not type the callable.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>, part: Part) -> Option<Parametric> {
        let born = self.born(body.segment())?;
        let map = match born.callable {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        let (types, scratch) = (self.types, self.scratch);
        let ktype = in_body_of(born.callable, born.levels, types, scratch)?;
        let TypeNode::KFunction { params, ret, .. } = types.node(ktype) else {
            return None;
        };
        let read = match part {
            Part::Return => ret,
            Part::Parameter(name) => params.get(name)?,
        };
        let mut named = BumpVec::new_in(scratch);
        read_through(types, scratch, read, Side::Above, &mut |variable| {
            if let Variable::Lexical { level, .. } = variable {
                named.push(level);
            }
            None
        });
        // A name the binding binds reads as the closed type it binds; any other is gathered.
        let mut closed = BumpVec::new_in(scratch);
        for (name, index) in map.iter() {
            let level = born.levels.get(index).map(|at| types.node(*at));
            let Some(TypeNode::Lexical { level, .. }) = level else {
                continue;
            };
            if named.contains(&level)
                && let Some((slot, _)) = body.slot(BinderSymbol::Type(name))
            {
                let unknown = Unknown {
                    segment: body.segment(),
                    at: Address::Slot(slot),
                };
                match self.binds(unknown) {
                    Some(bound) => closed.push((level, bound)),
                    None => self.note(&[unknown]),
                }
            }
        }
        if closed.is_empty() {
            return Some(read);
        }
        Some(read_through(
            types,
            scratch,
            read,
            Side::Above,
            &mut |variable| {
                let Variable::Lexical { level, .. } = variable else {
                    return None;
                };
                let (_, bound) = closed.iter().find(|(held, _)| *held == level)?;
                Some(Interval::point((*bound).into()))
            },
        ))
    }

    /// The `part` of the function type of the callable body at `level` as its body reads it: its
    /// own group instantiated at its group levels, its paths read through their resolutions.
    /// `None` where the walk did not type the callable, or a path it reads is not resolved.
    pub(super) fn in_body(&self, level: usize, part: Part) -> Option<Parametric> {
        let shape = self.chain[level].shape;
        let read = self.body_type(shape, part)?;
        match self.callable_cell(shape).over_paths() {
            true => self.through_paths(level, read),
            false => Some(read),
        }
    }

    /// The solution the callable body `body` is born instantiated at, where the walk fixed one.
    pub(super) fn born_instance(&self, body: &BodyShape<'graph>) -> Option<StaticSolution<'o>> {
        self.born(body.segment())?.instance
    }

    /// Fix the solution the callable body `body` is born instantiated at, in the unit birthing it.
    pub(super) fn fix_born_instance(
        &mut self,
        body: &'graph BodyShape<'graph>,
        solution: StaticSolution<'o>,
    ) {
        let segment = body.segment();
        let born = self.born(segment).unwrap_or(Born {
            callable: Static::Unknown,
            levels: &[],
            instance: None,
            digestible: &[],
        });
        self.set_born(
            segment,
            Born {
                instance: Some(solution),
                ..born
            },
        );
    }

    /// The summary of the callable body `body`, where the walk closed it: `Some(None)` where it has
    /// none.
    pub(super) fn summary_of(&self, body: &BodyShape<'graph>) -> Option<Option<Tree<'o>>> {
        self.closed(body.segment())?.summary
    }

    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where the walk typed it
    /// exactly.
    pub(super) fn signature_of(&self, body: &BodyShape<'graph>) -> Option<KType> {
        match self.level_of(body.segment()) {
            Some(level) => self.chain[level].signature,
            None => self.inline_facts(body.segment(), |facts| facts.signature)?,
        }
    }

    /// The last statement's static type of the quote's code `code` as it runs unfilled.
    pub(super) fn ran_of(&self, code: &BodyShape<'graph>) -> Option<Interval> {
        self.closed(code.segment())?.ran
    }

    /// Whether the closing record of `body` stands for this walk: one it wrote, or its base's where
    /// the binding wakes nothing that record waits on — a body that re-runs is walked before its
    /// record is read, so a cycle a binding closes is found (F3). A test of progress, which gathers
    /// nothing.
    pub(super) fn has_closed(&self, body: &BodyShape<'graph>) -> bool {
        let segment = body.segment();
        self.store.close(segment).is_some_and(|(_, frontier)| {
            self.walked[segment.index() as usize] || !self.store.wakes(frontier)
        })
    }

    /// Whether the walk has walked `nested`, or need not: a quote's code the shape refused is left.
    pub(super) fn done(&self, nested: &BodyShape<'graph>) -> bool {
        self.walked[nested.segment().index() as usize]
            || (nested.kind() == ShapeKind::Code && nested.refusal().is_some())
    }

    /// The unknown the binder at `slot` of the shape at `level` is, where it is a module binder:
    /// a binding binds it to a closed content tree, so every fact reading its content waits on it.
    pub(super) fn module_unknown(&self, level: usize, slot: Slot, bound: Bound) -> Option<Unknown> {
        let shape = self.chain[level].shape;
        let (BinderSymbol::Value(_), DeclaredType::Type(typed)) = (shape.slot_name(slot), bound)
        else {
            return None;
        };
        let (types, scratch) = (self.types, self.scratch);
        let upper = bound_above(types, scratch, typed.upper);
        let signature = match types.node(upper) {
            TypeNode::Signature { .. } => upper,
            TypeNode::SignatureApply { signature, .. } => signature,
            _ => return None,
        };
        schema_of(signature, types)?;
        Some(Unknown {
            segment: shape.segment(),
            at: Address::Slot(slot),
        })
    }

    /// Gather the unknown the binder at `slot` of the shape at `level`, of static type `bound`,
    /// is itself, where it is one: a type name the chain numbers and no closed declaration fixes,
    /// or a module binder.
    pub(super) fn note_own(&self, level: usize, slot: Slot, bound: Bound) {
        let shape = self.chain[level].shape;
        let homed = (self.shapes.level(shape.segment(), Address::Slot(slot))).is_some();
        let closed = matches!(
            self.chain[level].declared[slot.index()],
            Some((Static::Closed(_), _))
        );
        let own = match homed && !closed {
            true => Some(self.unknown_at(level, slot)),
            false => self.module_unknown(level, slot, bound),
        };
        if let Some(own) = own {
            self.note(&[own]);
        }
    }

    /// How many closure slots a birth of `body` fills: its builder captures, then its layout.
    pub(super) fn capture_count(&self, body: &BodyShape<'graph>) -> usize {
        body.captures().len() + self.shapes.layout(body.segment()).len()
    }

    /// Whether `body`'s content composes its closure slot `capture`: a builder capture reading no
    /// top-level slot, or a type capture the contract names so far.
    pub(super) fn composes(
        &self,
        body: &BodyShape<'graph>,
        capture: crate::scope::CaptureSlot,
    ) -> bool {
        match capture.index().checked_sub(body.captures().len()) {
            None => body.composes(capture),
            Some(index) => self.composes_type(body, index),
        }
    }

    /// Whether `body`'s content composes its type capture at `index` past its builder captures:
    /// one a contribution or a solution read from inside it names, homed outside it.
    pub(super) fn composes_type(&self, body: &BodyShape<'graph>, index: usize) -> bool {
        let layout = self.shapes.layout(body.segment());
        layout.get(index).is_some_and(|(level, _)| {
            self.composed[body.segment().index() as usize].contains(level)
        })
    }
}

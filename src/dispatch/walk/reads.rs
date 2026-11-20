//! The walk's **façade**: the one door every read and write of a fact goes through, so a rule
//! never names where a fact rests ([the store](../../build/README.md#the-store)).
//!
//! A fact the walk fixed rests in its shape's level while the shape is on the chain, beside the
//! unit that fixed it, and once it leaves the chain in that unit's record, an inline body's as an
//! owning body's; a module body's signature in the record of the unit that ran it; a callable's or
//! a quote's code's birth facts in its birth unit's; a body's closing facts in its closing record.
//! A **standing** shape's — one a rooted walk stands around its root — rest in its records alone:
//! each is read off the record of the unit holding it, at or before the unit birthing the next
//! level, which the walk tags at its first read ([`stood`](Pass::stood)).
//!
//! A read gathers what the fact waits on, at unit grain: nothing for a fact of the unit running,
//! the gathering so far of a unit still running around it, and a read of the frontier of one that
//! ended or was only read, cut to the chain. An entry unit seeds each binder in a gathering of its
//! own, its **seed**, and a read of a binder gathers its seed alone. Only an unknown of a shape on
//! the chain is gathered, and one the binding binds is closed. A record notes each variable its
//! types name ([`note_named`](Pass::note_named)); a door notes what it judged open.
//!
//! A record the binding names but does not wake is read closed ([`standing_record`]
//! (Pass::standing_record)): each variable the binding binds written in by substitution, the record
//! laid again only where a type changes, and each type closed whole kept for the layer's table of
//! closed types.

use crate::elaborate::judge::{self, Marks, free_variables};
use crate::elaborate::schema_of;
use crate::memory::BumpVec;
use crate::memory::{BumpBackedMap, bump_table, resident};
use crate::scope::{
    Address, Answer, Arms, BodyShape, Born, CloseLaying, Closed, Closer, Closing, Cut, DerivedId,
    DerivedTerm, Frontier, Held, Holder, Identity, InstanceRead, Mark, Narrowing, Noted, OpenKind,
    OpenMark, Pick, Points, Question, Restamp, Route, Segment, ShapeError, ShapeKind, Site, Slot,
    Static, StaticCallable, StaticInstance, StaticRegistered, StaticType, Store, Traced, Tree,
    Typed, UnitFacts, UnitId, UnitIndex, Unknown, UseStatics, Wake, in_body_of,
};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Record, Side, TypeNode, Variable,
};

use super::trees::Substitution;
use super::{Bound, Pass, READ, Tag, Wrote};

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
        if tag.0 & READ != 0 {
            let (unit, frontier) = self.read.borrow().units[(tag.0 & !READ) as usize];
            return self.note_frontier(frontier, unit.segment);
        }
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
        let before = inner.noted.len() + inner.reads.len();
        (inner.noted).extend(
            (outer.noted.iter())
                .copied()
                .filter(|noted| self.gathers(noted.unknown)),
        );
        // A read kept what lies at or above its cut; of that, what is still on the chain lies at
        // or above the innermost body on the chain enclosing the cut.
        inner.reads.extend(outer.reads.iter().map(|cut| Cut {
            frontier: cut.frontier,
            from: self.meet(cut.from),
        }));
        #[cfg(test)]
        super::count(|counts| counts.noted += inner.noted.len() + inner.reads.len() - before);
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
    pub(super) fn meet(&self, mut segment: Segment) -> Segment {
        while self.level_of(segment).is_none() {
            segment = (self.shapes.parent(segment))
                .expect("the program encloses every body")
                .0;
        }
        segment
    }

    /// Add `unknowns` to the innermost gathering, each only named, unsorted: its frontier sorts
    /// and dedups it once, at `end`.
    pub(super) fn note(&self, unknowns: &[Unknown]) {
        if unknowns.is_empty() {
            return;
        }
        self.noting(unknowns.iter().copied().map(Noted::named));
    }

    /// Add `unknown` to the innermost gathering, open by `mark`: a judgement over it reached no
    /// answer that holds at every binding within its ends.
    pub(super) fn note_open(&self, unknown: Unknown, mark: OpenMark<'o>) {
        self.noting(std::iter::once(Noted {
            unknown,
            mark: Mark::Open(mark),
        }));
    }

    /// The unknown the lexical variable at `level` is, read from the innermost shape on the chain:
    /// the first body outward from it that homes the level. A type the walk judges names only
    /// levels its chain homes, each homed once along it.
    fn unknown_of_level(&self, level: usize) -> Option<Unknown> {
        self.shapes
            .unknown_of_level(self.chain.last()?.shape.segment(), level)
    }

    /// Hand `each` the unknowns `variable` is, read from `segment`: for a lexical variable the
    /// one homing its level, and for a
    /// [derived name](crate::scope::DerivedName) those of every variable its term reads,
    /// transitively, or the `:Module` binder whose carried type it is. A mark on a derived name is a mark on each name its term reads, and one on a
    /// path class's variable a mark on the module binder its paths read through.
    pub(super) fn each_unknown_of(
        &self,
        segment: Segment,
        variable: Variable,
        each: &mut dyn FnMut(Unknown),
    ) {
        match variable {
            Variable::Lexical { level, .. } => {
                if let Some(unknown) = self.shapes.unknown_of_level(segment, level) {
                    each(unknown);
                }
            }
            Variable::Derived { term, .. } => {
                let Some(name) = self.store.derived_name(term) else {
                    return;
                };
                if let DerivedTerm::Carried { binder, .. } = name.term {
                    // A capture crossing into code is its source wherever a binding binds that.
                    let mut binder = Some(binder);
                    while let Some(unknown) = binder {
                        each(unknown);
                        binder = self.shapes.crossed(unknown);
                    }
                    return;
                }
                name.term.each_type(&mut |ty| {
                    for variable in free_variables(self.types, self.scratch, ty) {
                        self.each_unknown_of(segment, variable, each);
                    }
                });
            }
            Variable::Path { class, .. } => self.class_binders(class, each),
            _ => {}
        }
    }

    /// The closed type the binding gives the lexical variable at `level`, read from `segment`: a
    /// bound type name's.
    fn level_bound(&self, segment: Segment, level: usize) -> Option<KType> {
        let unknown = self.shapes.unknown_of_level(segment, level)?;
        match self.bound_to(unknown)? {
            crate::scope::Bound::Type(bound) => Some(bound),
            crate::scope::Bound::Module { .. } => None,
        }
    }

    /// Add each of `noted` the walk gathers to the innermost gathering.
    fn noting(&self, noted: impl Iterator<Item = Noted<'o>>) {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            #[cfg(test)]
            let before = open.noted.len();
            open.noted
                .extend(noted.filter(|noted| self.gathers(noted.unknown)));
            #[cfg(test)]
            super::count(|counts| counts.noted += open.noted.len() - before);
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
            open.noted.retain(|noted| noted.unknown.segment != segment);
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

    /// What the binding binds `unknown` to, where it binds it or, for a capture crossing into
    /// code, its source ([`Shapes::crossed`](crate::scope::Shapes::crossed)), as a level is bound.
    pub(super) fn bound_through(&self, mut unknown: Unknown) -> Option<crate::scope::Bound<'o>> {
        loop {
            match self.bound_to(unknown) {
                Some(bound) => return Some(bound),
                None => unknown = self.shapes.crossed(unknown)?,
            }
        }
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
        Some(self.standing_record(unit, facts, frontier))
    }

    /// `facts`, the record of `unit` the store holds beside `frontier`, as this walk reads it: closed
    /// under the binding where it stands named ([`Wake::Named`]), else as it rests.
    pub(super) fn standing_record(
        &self,
        unit: UnitId,
        facts: &'o UnitFacts<'o>,
        frontier: Frontier<'o>,
    ) -> &'o UnitFacts<'o> {
        if self.store.wakes(frontier, self.types) != Wake::Named {
            return facts;
        }
        let at = std::ptr::from_ref(facts).addr();
        if let Some(closed) = self.rebound.borrow().units.get(&at) {
            return closed;
        }
        let closed = facts.closed_laying(&mut Rebinding {
            pass: self,
            segment: unit.segment,
        });
        self.rebound.borrow_mut().units.insert(at, closed);
        closed
    }

    /// Substitute the walk's binding into each open tree `facts`, a record the walk absorbs, holds,
    /// before the record is read closed: the tree a fresh load builds under the binding, entered
    /// in the walk's table of trees where it differs ([`Closer`]'s trees). A view that shows a
    /// member its substituted source hides refuses at its `:|`, as a fresh load does.
    pub(super) fn substitute_trees(
        &mut self,
        facts: &'o UnitFacts<'o>,
    ) -> Result<(), ShapeError<'o>> {
        let mut done = bump_table(self.scratch);
        for (segment, typed) in facts.typed.iter() {
            let shape = self.shape_at(*segment);
            // A content tree is substituted at the site a fresh load reads it from.
            let contents = (typed.contents.iter()).filter_map(|(slot, tree)| {
                let site = match (shape.rhs(*slot), shape.birth_site(*slot)) {
                    (Some(rhs), _) => shape.site(rhs),
                    (None, born) => born?,
                };
                Some((site, *tree, false))
            });
            let views = typed.trees.iter().map(|(site, tree)| (*site, **tree, true));
            for (site, tree, view) in views.chain(contents) {
                let identity = tree.identity();
                if !matches!(identity, Identity::Open(_))
                    || self.rebound.borrow().trees.contains_key(&identity)
                {
                    continue;
                }
                let substituted =
                    self.substituted(tree, Substitution::Binding(*segment), site, &mut done)?;
                if view
                    && let Tree::View {
                        application,
                        source,
                        ..
                    } = substituted
                    // Bindings are cumulative, so the one closing the application checks it.
                    && let Some(ascribed) = judge::static_of(self.types, application)
                    && let (_, _, Some(member)) = self.view_tree_held(application, *source, site)?
                {
                    // The `:|`'s own node, whose site is its first part's, as a fresh load refuses
                    // at it.
                    let mut at = None;
                    for statement in shape.body() {
                        if shape.site_of_node(statement) == site {
                            at = Some(statement.source);
                        }
                        crate::scope::each_part(statement, |part| {
                            if let crate::parse::ExpressionPart::Expression(viewing) = part
                                && !viewing.parts.is_empty()
                                && shape.site_of_node(viewing) == site
                            {
                                at = Some(viewing.source);
                            }
                        });
                    }
                    let at = at.expect("a view's site lies in its shape's statements");
                    return Err(ShapeError::ViewReveals {
                        member: member.symbol(),
                        ascribed,
                        at,
                    });
                }
                if substituted.identity() != identity {
                    let substituted = resident(self.lay, substituted);
                    self.rebound
                        .borrow_mut()
                        .trees
                        .insert(identity, substituted);
                }
            }
        }
        Ok(())
    }

    /// `closing`, the closing record of `segment` the store holds beside `frontier`, as this walk
    /// reads it: closed under the binding where it stands named, as a unit record is.
    pub(super) fn standing_closing(
        &self,
        segment: Segment,
        closing: &'o Closing<'o>,
        frontier: Frontier<'o>,
    ) -> &'o Closing<'o> {
        if self.store.wakes(frontier, self.types) != Wake::Named {
            return closing;
        }
        let at = std::ptr::from_ref(closing).addr();
        if let Some(closed) = self.rebound.borrow().closings.get(&at) {
            return closed;
        }
        let closed = closing.closed_laying(&mut Rebinding {
            pass: self,
            segment,
        });
        self.rebound.borrow_mut().closings.insert(at, closed);
        closed
    }

    /// `ty`, named by a record of `segment`'s, under the binding: each lexical variable it names
    /// that the binding binds, read from `segment`, written in. Each type closed, whole or in part,
    /// is an entry of the layer's table.
    pub(super) fn rebind(&self, segment: Segment, ty: DeclaredType<Parametric>) -> Closed {
        if let Some(closed) = self.rebound.borrow().types.get(&(segment, ty)) {
            return *closed;
        }
        let rebinding = Rebinds {
            pass: self,
            segment,
        };
        let closed = crate::scope::rebound(self.types, self.scratch, ty, &rebinding);
        let mut rebound = self.rebound.borrow_mut();
        rebound.types.insert((segment, ty), closed);
        if !matches!(closed, Closed::Same) {
            // One handle names one unknown along a chain, whose bodies hold disjoint levels, so
            // every segment closing it closes it alike (`closed.rs`).
            let held = *rebound.table.entry(ty).or_insert(closed);
            debug_assert!(
                held == closed,
                "a type closes alike from every segment of a chain"
            );
        }
        closed
    }

    /// A path of `class` under the chain root `root`, beside its resolution, as the record holding
    /// the class rests: what the class reads by its content, which no binding changes but for the
    /// application's names, read where it is used. Gathers nothing.
    pub(super) fn class_held(
        &self,
        root: Segment,
        class: crate::type_lattice::PathClass,
    ) -> Option<(crate::scope::PathEntry<'o>, crate::scope::Resolution<'o>)> {
        // The record holding the class: a newer record of its unit may type its root otherwise.
        let (_, facts) = self.store.holding(crate::scope::Held::Class(root, class))?;
        let (_, (entry, resolution)) =
            (facts.paths.iter()).find(|((at, _), (_, resolution))| {
                *at == root && resolution.resolved == crate::scope::Resolved::Class(class)
            })?;
        Some((*entry, *resolution))
    }

    /// The record of `unit` the store holds, beside the tag a read of one of its facts gathers:
    /// one per unit, made at its first read. For a unit the walk neither runs nor absorbs.
    pub(super) fn read_unit(&self, unit: UnitId) -> Option<(Tag, &'o UnitFacts<'o>)> {
        let (facts, frontier) = self.store.fact(unit)?;
        let facts = self.standing_record(unit, facts, frontier);
        Some((self.read_tag(unit, None, frontier), facts))
    }

    /// The tag a read of `frontier` gathers — the record of `unit`, or its seed of the binder
    /// `seed` — made at its first read.
    fn read_tag(&self, unit: UnitId, seed: Option<Slot>, frontier: Frontier<'o>) -> Tag {
        let mut read = self.read.borrow_mut();
        match read.tags.get(&(unit, seed)) {
            Some(tag) => *tag,
            None => {
                let tag = Tag(READ | read.units.len() as u32);
                read.units.push((unit, frontier));
                read.tags.insert((unit, seed), tag);
                tag
            }
        }
    }

    /// The record of the unit `unit` of the standing shape at `level`, beside the tag a read of it
    /// gathers: `None` past the level's birth unit, whose facts are not fixed when the run reaches
    /// the birth. A standing level holds no fact of its own, so every read of one comes here.
    pub(super) fn stood_record(
        &self,
        level: usize,
        unit: UnitIndex,
    ) -> Option<(Tag, &'o UnitFacts<'o>)> {
        let birth = self.chain[level]
            .standing
            .expect("only a standing level is read off its records");
        if unit > birth {
            return None;
        }
        #[cfg(test)]
        super::count(|counts| counts.stood += 1);
        let segment = self.chain[level].shape.segment();
        let id = UnitId { segment, unit };
        debug_assert!(
            (self.store.fact(id))
                .is_none_or(|(_, frontier)| !self.store.checks_open(frontier, self.types)),
            "a binding below the chain wakes none of its units"
        );
        self.read_unit(id)
    }

    /// The facts the unit `unit` of the standing shape at `level` fixed of it, beside the tag a
    /// read of them gathers, as [`stood_record`](Self::stood_record) reads them.
    pub(super) fn stood(&self, level: usize, unit: UnitIndex) -> Option<(Tag, &'o Typed<'o>)> {
        let segment = self.chain[level].shape.segment();
        let (tag, facts) = self.stood_record(level, unit)?;
        facts.typed.get(segment).map(|(_, typed)| (tag, typed))
    }

    /// [`stood`](Self::stood) for the unit holding the fact at `site` of the standing shape at
    /// `level`.
    fn stood_site(&self, level: usize, site: Site) -> Option<(Tag, &'o Typed<'o>)> {
        let segment = self.chain[level].shape.segment();
        let (unit, _) = self.store.holding(Held::Site(segment, site))?;
        self.stood(level, unit.unit)
    }

    /// [`stood`](Self::stood) for the unit holding the slot `slot`'s facts.
    fn stood_slot(&self, level: usize, slot: Slot) -> Option<(Tag, &'o Typed<'o>)> {
        let segment = self.chain[level].shape.segment();
        self.stood(level, self.shapes.slot_unit(segment, slot).unit)
    }

    /// The content tree the record of the unit binding the module binder at `slot` of the
    /// standing shape at `level` holds, beside the tag a read of it gathers.
    pub(super) fn stood_content(&self, level: usize, slot: Slot) -> Option<(Tree<'o>, Tag)> {
        if !self.standing(level) {
            return None;
        }
        let (tag, typed) = self.stood_slot(level, slot)?;
        typed.contents.get(slot).map(|(_, tree)| (*tree, tag))
    }

    /// The content tree of the module binder at `slot` of the standing shape at `level`, beside the
    /// tag a read of it gathers, where its record says it holds a module of `identity`'s tree.
    pub(super) fn stood_holding(
        &self,
        level: usize,
        slot: Slot,
        identity: Identity,
    ) -> Option<(Tree<'o>, Tag)> {
        let (tag, typed) = self.stood_slot(level, slot)?;
        let (_, held) = typed.holding.get(slot)?;
        if !held.contains(&identity) {
            return None;
        }
        typed.contents.get(slot).map(|(_, tree)| (*tree, tag))
    }

    /// Whether the shape at `level` is standing: read off its records, never walked.
    pub(super) fn standing(&self, level: usize) -> bool {
        self.chain[level].standing.is_some()
    }

    /// The state of `unit` in this walk, where it ran or absorbed it.
    fn state_of(&self, unit: UnitId) -> Option<(Tag, &super::UnitState<'p, 'o>)> {
        let tag = *self.started.get(&unit)?;
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

    /// Read `read` off the facts the unit that ran `segment`, an inline body off the chain, keeps
    /// of it — its signature — its frontier gathered.
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

    /// Read `read` off the facts of `segment`, a body off the chain, that `unit` of it fixed, its
    /// frontier gathered.
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
    pub(super) fn off_chain_slot<R>(
        &self,
        segment: Segment,
        slot: Slot,
        read: impl FnOnce(&Typed<'o>) -> R,
    ) -> Option<R> {
        self.owner_facts(segment, self.shapes.slot_unit(segment, slot), read)
    }

    /// The static type of the binder at `slot` of the shape at `level`, as the walk has it now,
    /// and the binder itself where it is a module binder a binding may pin.
    pub(super) fn binder_at(&self, level: usize, slot: Slot) -> Bound {
        self.binder_holding(level, slot, None)
    }

    /// [`binder_at`](Self::binder_at) for a binder whose content tree the reader holds as `tree`,
    /// where the walk may not have fixed it yet.
    pub(super) fn binder_holding(&self, level: usize, slot: Slot, tree: Option<Tree<'o>>) -> Bound {
        let (bound, tag) = self.binder_held(level, slot);
        if let Some(tag) = tag {
            self.gather(tag);
        }
        if let Some(unknown) = self.module_unknown(level, slot, bound) {
            self.note_module(unknown, bound, tree);
        }
        bound
    }

    /// Gather the module binder `unknown`, of static type `bound`, its content tree `tree` where
    /// the reader holds it: named where `bound` is exact over its own paths, or the type it carries,
    /// alone, which a binding of it closes to the type its retype stamps, the type it binds to;
    /// else open, as a read of its content, but for a local the run never binds.
    fn note_module(&self, unknown: Unknown, bound: Bound, tree: Option<Tree<'o>>) {
        let own = match bound {
            DeclaredType::Type(typed) if typed.lower == typed.upper => {
                let upper = DeclaredType::Type(typed.upper);
                (free_variables(self.types, self.scratch, upper).iter()).all(|variable| {
                    match *variable {
                        Variable::Path { class, .. } => self.class_binder(class) == Some(unknown),
                        Variable::Derived { term, .. } => matches!(self.term(term),
                            Some(DerivedTerm::Carried { binder, .. }) if binder == unknown),
                        _ => false,
                    }
                })
            }
            _ => false,
        };
        if own {
            return self.note_named_unknown(unknown);
        }
        // A local the run never binds marks nothing: one whose content the load built closed, or
        // one whose tree a binding of the module binders its leaves read gives whole, its type
        // over the paths through that tree, which those bindings close.
        match tree.or_else(|| self.local_content(unknown)) {
            Some(tree) if tree.closed() && !matches!(tree, Tree::Leaf(_)) => {}
            Some(tree) if self.substitutes(tree) => self.note_named_of(tree),
            _ => self.note_content(unknown, bound),
        }
    }

    /// The content tree the walk holds for `unknown`, a module binder no parameter: the newest a
    /// unit of a level the walk runs fixed, else the one its own unit read off its right-hand side
    /// before fixing it, else a standing level's record's.
    fn local_content(&self, unknown: Unknown) -> Option<Tree<'o>> {
        let Address::Slot(slot) = unknown.at else {
            return None;
        };
        let level = self.level_of(unknown.segment)?;
        if self.chain[level].shape.position_of(slot) == crate::scope::Position::PARAMETER {
            return None;
        }
        match self.standing(level) {
            true => self.stood_content(level, slot).map(|(tree, _)| tree),
            false => (self.holding.get(&(unknown.segment, Holder::Slot(slot))))
                .map(|(tree, _, _)| *tree)
                .or_else(|| {
                    let shape = super::address(self.chain[level].shape);
                    (self.binders.get(&(shape, slot, None))).map(|(tree, _)| *tree)
                }),
        }
    }

    /// Gather the binder at `slot` of the shape at `level` named where a content tree holds a leaf
    /// on it: a module binder the binding leaves unbound, whose content a binding gives the tree
    /// by tree substitution as the walk absorbs the record holding it.
    pub(super) fn note_leaf(&self, level: usize, slot: Slot) {
        let (bound, _) = self.binder_held(level, slot);
        if let Some(unknown) = self.module_unknown(level, slot, bound)
            && self.binds(unknown).is_none()
        {
            self.note_named_unknown(unknown);
        }
    }

    /// Gather open each module binder of the chain a leaf of `tree` reads in the run being typed,
    /// where the binding leaves it unbound: a type read off `tree` is one a binding of the binder
    /// changes, past what tree substitution gives.
    pub(super) fn note_content_of(&self, tree: Tree<'o>) {
        for binder in self.leaf_binders(tree) {
            if let Some(level) = self.level_of(binder.body) {
                let (bound, _) = self.binder_held(level, binder.slot);
                if let Some(unknown) = self.module_unknown(level, binder.slot, bound)
                    && self.binds(unknown).is_none()
                {
                    self.note_content(unknown, bound);
                }
            }
        }
    }

    /// Gather named each module binder of the chain a leaf of `tree` reads in the run being typed,
    /// where the binding leaves it unbound: a binding of it substitutes the tree.
    pub(super) fn note_named_of(&self, tree: Tree<'o>) {
        for binder in self.leaf_binders(tree) {
            if let Some(level) = self.level_of(binder.body) {
                let (bound, _) = self.binder_held(level, binder.slot);
                if let Some(unknown) = self.module_unknown(level, binder.slot, bound)
                    && self.binds(unknown).is_none()
                {
                    self.note_named_unknown(unknown);
                }
            }
        }
    }

    /// Whether a binding of the module binders the leaves of `tree` read gives its content whole:
    /// an open tree each of whose leaves is on such a binder of the chain.
    pub(super) fn substitutes(&self, tree: Tree<'o>) -> bool {
        let binders = self.leaf_binders(tree);
        matches!(tree.identity(), Identity::Open(_))
            && !binders.is_empty()
            && binders.iter().all(|binder| {
                self.level_of(binder.body).is_some_and(|level| {
                    let (bound, _) = self.binder_held(level, binder.slot);
                    self.module_unknown(level, binder.slot, bound).is_some()
                })
            })
    }

    /// Each binder a leaf of `tree` reads in the run being typed, once, memoized by identity: a
    /// tree shares its subtrees, so a chain of views over one leaf reads it once.
    pub(super) fn leaf_binders(&self, tree: Tree<'o>) -> &'p [crate::scope::Binder] {
        let identity = tree.identity();
        if tree.closed() {
            return &[];
        }
        if let Some(binders) = self.leaf_binders.borrow().get(&identity) {
            return binders;
        }
        let mut binders = BumpVec::new_in(self.scratch);
        if let Tree::Leaf(crate::scope::Leaf::Bound {
            binder,
            within: None,
            ..
        }) = tree
        {
            binders.push(binder);
        }
        tree.subtrees(&mut |subtree| {
            for binder in self.leaf_binders(*subtree) {
                if !binders.contains(binder) {
                    binders.push(*binder);
                }
            }
        });
        let binders: &'p [crate::scope::Binder] =
            crate::memory::collect(self.scratch, binders.iter().copied());
        if identity != Identity::Unnamed {
            self.leaf_binders.borrow_mut().insert(identity, binders);
        }
        binders
    }

    /// Gather `unknown` named: a fact the walk wrote over it closes under a binding of it.
    pub(super) fn note_named_unknown(&self, unknown: Unknown) {
        self.noting(std::iter::once(Noted::named(unknown)));
    }

    /// Note named each lexical variable the types `visit` hands out hold, each read from
    /// `segment`: a record names them, so a binding of one reads it closed.
    pub(super) fn note_named(
        &self,
        segment: Segment,
        visit: impl FnOnce(&mut dyn FnMut(DeclaredType<Parametric>)),
    ) {
        let mut named = BumpVec::new_in(self.scratch);
        visit(&mut |ty| {
            for variable in free_variables(self.types, self.scratch, ty) {
                self.each_unknown_of(segment, variable, &mut |unknown| {
                    named.push(Noted::named(unknown));
                });
            }
        });
        self.noting(named.into_iter());
    }

    /// Gather the module binder `unknown`, of static type `bound`, open: every read of a module
    /// binder is a read of its content, which a binding pins.
    pub(super) fn note_content(&self, unknown: Unknown, bound: Bound) {
        let judged = match bound {
            DeclaredType::Type(typed) => DeclaredType::Type(typed.upper),
            DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
        };
        let mark = OpenMark::undecided(OpenKind::ModuleContent, None, judged);
        self.note_open(unknown, mark);
    }

    /// Log that the unit `tag` wrote at `wrote` in the shape at `level`, where that shape is the
    /// unit's own body: its record lays what it wrote.
    fn wrote(&mut self, level: usize, tag: Tag, wrote: Wrote) {
        let segment = self.chain[level].shape.segment();
        let owner = self.owner_of(tag);
        let state = &mut self.units[owner.0 as usize];
        if state.id.segment == segment {
            state.wrote.push(wrote);
        }
    }

    /// The static type of the binder at `slot` of the shape at `level`, read without gathering it.
    pub(super) fn binder_fact(&self, level: usize, slot: Slot) -> Bound {
        self.binder_held(level, slot).0
    }

    /// The static type of the binder at `slot` of the shape at `level`, beside the unit that fixed
    /// it: `[Never, Any]` where none has.
    fn binder_held(&self, level: usize, slot: Slot) -> (Bound, Option<Tag>) {
        if !self.standing(level) {
            return self.chain[level].binders[slot.index()];
        }
        // A seeded binder's read gathers its seed alone, as on the chain.
        let segment = self.chain[level].shape.segment();
        let unit = self.shapes.slot_unit(segment, slot);
        let held = self
            .stood_record(level, unit.unit)
            .and_then(|(tag, facts)| {
                let (_, typed) = facts.typed.get(segment)?;
                let (_, bound) = typed.binders.get(slot)?;
                let tag = match facts.seeds.get(slot) {
                    Some((_, seed)) => self.read_tag(unit, Some(slot), *seed),
                    None => tag,
                };
                Some((*bound, Some(tag)))
            });
        held.unwrap_or((DeclaredType::Type(crate::values::unknown()), None))
    }

    /// Fix the static type of the binder at `slot` of the shape at `level`.
    pub(super) fn set_binder(&mut self, level: usize, slot: Slot, bound: Bound) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.open.last().copied();
        self.chain[level].binders[slot.index()] = (bound, tag);
        if let Some(tag) = tag {
            self.wrote(level, tag, Wrote::Binder(slot));
        }
    }

    /// The static type of statement `index` of the shape at `level`, as the walk has it now.
    pub(super) fn statement_at(&self, level: usize, index: usize) -> Interval {
        let held = match self.standing(level) {
            false => self.chain[level].statements[index],
            true => {
                let segment = self.chain[level].shape.segment();
                let unit = self.shapes.running(segment, index as u32).unit;
                self.stood(level, unit).and_then(|(tag, typed)| {
                    (typed.statements.get(index as u32)).map(|(_, typed)| (*typed, tag))
                })
            }
        };
        match held {
            Some((typed, tag)) => {
                self.gather(tag);
                typed
            }
            None => crate::values::unknown(),
        }
    }

    /// The static type of the last statement of the shape at `level`, where it has one.
    pub(super) fn last_statement(&self, level: usize) -> Option<Interval> {
        let last = self.chain[level].shape.body().len().checked_sub(1)?;
        Some(self.statement_at(level, last))
    }

    /// Fix the static type of statement `index` of the shape at `level`.
    pub(super) fn set_statement(&mut self, level: usize, index: usize, typed: Interval) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        self.chain[level].statements[index] = Some((typed, tag));
        self.wrote(level, tag, Wrote::Statement(index));
    }

    /// The static type the walk recorded for the part at `site` of the shape at `level`, its
    /// latest where it recorded more than one.
    pub(super) fn part_type(&self, level: usize, site: Site) -> Option<Interval> {
        let (typed, tag) = self.part_held(level, site)?;
        self.gather(tag);
        Some(typed)
    }

    /// The static type the walk recorded for the part at `site` of the shape at `level`, beside the
    /// unit that recorded it, read without gathering it.
    pub(super) fn part_held(&self, level: usize, site: Site) -> Option<(Interval, Tag)> {
        if !self.standing(level) {
            return self.chain[level].parts.get(&site).copied();
        }
        let (tag, typed) = self.stood_site(level, site)?;
        typed.parts.get(site).map(|(_, typed)| (*typed, tag))
    }

    /// Record the static type of the part at `site` of the shape at `level`.
    pub(super) fn record_part(&mut self, level: usize, site: Site, typed: Interval) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        self.chain[level].parts.insert(site, (typed, tag));
        self.wrote(level, tag, Wrote::Part(site));
    }

    /// What the walk fixed about the keyworded use at `index` of the shape at `level`.
    pub(super) fn use_at(&self, level: usize, index: usize) -> UseStatics<'o> {
        let (fixed, tag) = match self.standing(level) {
            false => self.chain[level].uses[index],
            true => {
                let site = self.chain[level].shape.candidate_lists()[index].0;
                let held = self.stood_site(level, site).and_then(|(tag, typed)| {
                    (typed.uses.get(site)).map(|(_, fixed)| (*fixed, Some(tag)))
                });
                held.unwrap_or((UseStatics::UNFIXED, None))
            }
        };
        if let Some(tag) = tag {
            self.gather(tag);
        }
        fixed
    }

    /// Fix the narrowing of the keyworded use at `index` of the shape at `level`.
    pub(super) fn set_narrowing(&mut self, level: usize, index: usize, narrowing: Narrowing<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.narrowing = narrowing;
        at.1 = Some(tag);
        self.wrote(level, tag, Wrote::Use(index));
    }

    /// Record what an answer selects among at the keyworded use at `index` of the shape at
    /// `level`.
    pub(super) fn set_undecided(
        &mut self,
        level: usize,
        index: usize,
        undecided: &'o crate::scope::UndecidedCall<'o>,
    ) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.undecided = Some(undecided);
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
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.contributions = contributions;
        at.1 = Some(tag);
        self.wrote(level, tag, Wrote::Use(index));
    }

    /// The point each variable of the candidate selected at the use at `site` of the shape at
    /// `level` solves to, where the walk selected one there.
    pub(super) fn selection_at(&self, level: usize, site: Site) -> Option<Points<'o>> {
        if self.standing(level) {
            let (tag, typed) = self.stood_site(level, site)?;
            self.gather(tag);
            return typed.selections.get(site).map(|(_, points)| *points);
        }
        let at = &self.chain[level];
        if let Some(index) = at.shape.candidate_lists().position(site)
            && let (_, Some(tag)) = at.uses[index]
        {
            self.gather(tag);
        }
        at.selections.get(&site).map(|(points, _)| *points)
    }

    /// Record the points the candidate selected at the use at `site` solves to.
    pub(super) fn record_selection(&mut self, level: usize, site: Site, points: Points<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        self.chain[level].selections.insert(site, (points, tag));
        self.wrote(level, tag, Wrote::Selection(site));
    }

    /// Record what the selected use at `site` fixes of its callee's names.
    pub(super) fn record_traced(&mut self, level: usize, site: Site, traced: Traced<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        self.chain[level].traced.insert(site, (traced, tag));
        self.wrote(level, tag, Wrote::Traced(site));
    }

    /// Record the content tree of the view the `:|` at `site` builds.
    pub(super) fn record_view_tree(&mut self, level: usize, site: Site, tree: &'o Tree<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].trees.len();
        self.chain[level].trees.push((site, tree, tag));
        self.wrote(level, tag, Wrote::Tree(index));
        self.hold_view(self.chain[level].shape.segment(), site, tree, tag);
    }

    /// Record the picks of the view the `:|` at `site` builds.
    pub(super) fn record_picks(&mut self, level: usize, site: Site, picks: &'o [Pick<'o>]) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].picks.len();
        self.chain[level].picks.push((site, picks, tag));
        self.wrote(level, tag, Wrote::Picks(index));
    }

    /// Record the content tree of the module binder at `slot` of the shape at `level`.
    pub(super) fn record_content(&mut self, level: usize, slot: Slot, tree: Tree<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let identities = self.held_identities(tree);
        let index = self.chain[level].contents.len();
        self.chain[level]
            .contents
            .push((slot, tree, identities, tag));
        self.wrote(level, tag, Wrote::Content(index));
        self.hold(
            self.chain[level].shape.segment(),
            Holder::Slot(slot),
            tree,
            identities,
            tag,
        );
    }

    /// Record that the ascription or annotated binder's type part at `site` is settled.
    pub(super) fn record_settled(&mut self, level: usize, site: Site) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].settled.len();
        self.chain[level].settled.push((site, tag));
        self.wrote(level, tag, Wrote::Settled(index));
    }

    /// Record a minting site of the shape at `level`.
    pub(super) fn record_minting(&mut self, level: usize, site: Site) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
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
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
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
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].named.len();
        self.chain[level].named.push((site, contributions, tag));
        self.wrote(level, tag, Wrote::Named(index));
    }

    /// Record what the retype at `site` re-stamps at.
    pub(super) fn record_retype(&mut self, level: usize, site: Site, retype: Restamp) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].retypes.len();
        self.chain[level].retypes.push((site, retype, tag));
        self.wrote(level, tag, Wrote::Retype(index));
    }

    /// Record what the load fixed about the arms of the `MATCH` whose node sits at `site`.
    pub(super) fn record_arms(&mut self, level: usize, site: Site, arms: Arms<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].arms.len();
        self.chain[level].arms.push((site, arms, tag));
        self.wrote(level, tag, Wrote::Arms(index));
    }

    /// Record the route of the tail call whose node sits at `site`.
    pub(super) fn record_route(&mut self, level: usize, site: Site, route: Route<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        let index = self.chain[level].routes.len();
        self.chain[level].routes.push((site, route, tag));
        self.wrote(level, tag, Wrote::Route(index));
    }

    /// The type the walk fixed for the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'o> {
        let segment = shape.segment();
        if let Some(level) = self.level_of(segment)
            && !self.standing(level)
        {
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
        let typed = statement.and_then(|statement| {
            let unit = self.shapes.running(segment, statement);
            match self.level_of(segment) {
                Some(level) => self.stood(level, unit.unit).and_then(|(tag, typed)| {
                    self.gather(tag);
                    read(typed)
                }),
                None => self.owner_facts(segment, unit, read).flatten(),
            }
        });
        typed.unwrap_or(Static::Unknown)
    }

    /// Fix the type of the type expression at `site` of the shape at `level`.
    pub(super) fn set_expression(&mut self, level: usize, site: Site, typed: StaticType<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
        let tag = self.innermost();
        self.chain[level].expressions.insert(site, (typed, tag));
        self.wrote(level, tag, Wrote::Expression(site));
    }

    /// The declaration the walk fixed for the type binder at `slot` of `shape`, and the binder
    /// itself where it is unknown.
    pub(super) fn declared_cell(&self, shape: &BodyShape<'graph>, slot: Slot) -> StaticType<'o> {
        let segment = shape.segment();
        let typed = match self.level_of(segment) {
            Some(level) if self.standing(level) => (self.stood_slot(level, slot))
                .and_then(|(tag, typed)| {
                    self.gather(tag);
                    typed.declared.get(slot).map(|(_, typed)| *typed)
                })
                .unwrap_or(Static::Unknown),
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
        // A rigid declaration is read over the names it reads, which its reader gathers.
        if shape.declarations(slot).is_some() && matches!(typed, Static::Unknown) {
            self.note(&[Unknown {
                segment,
                at: Address::Slot(slot),
            }]);
        }
        typed
    }

    /// Fix the declaration of the type binder at `slot` of the shape at `level`.
    pub(super) fn set_declared(&mut self, level: usize, slot: Slot, typed: StaticType<'o>) {
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
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
            Some(level) if self.standing(level) => (self.stood_slot(level, slot))
                .and_then(|(tag, typed)| {
                    self.gather(tag);
                    typed.registered.get(slot).map(|(_, typed)| *typed)
                })
                .unwrap_or(Static::Unknown),
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
        debug_assert!(!self.standing(level), "a standing level fixes nothing");
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

    /// The parameters and return of `callable`'s function type as its body reads it, its own group
    /// instantiated at `levels`: `None` where the walk did not type the callable.
    pub(super) fn function_in_body(
        &self,
        callable: StaticCallable<'o>,
        levels: &[Parametric],
    ) -> Option<(Record<'tier, Parametric>, Parametric)> {
        // A callable's own type is a function, never a variable.
        let ktype = in_body_of(callable, levels, self.types, self.scratch)?;
        match judge::node_unless_variable(self.types, ktype) {
            Some(TypeNode::KFunction { params, ret }) => Some((params, ret)),
            _ => None,
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
        let (params, ret) = self.function_in_body(born.callable, born.levels)?;
        let read = match part {
            Part::Return => ret,
            Part::Parameter(name) => params.get(name)?,
        };
        let mut named = BumpVec::new_in(scratch);
        // Which of its own levels the part names: a scan, which reads no end.
        judge::read_points(types, scratch, read, Side::Above, &mut |variable| {
            if let Variable::Lexical { level, .. } = variable {
                named.push(level);
            }
            None
        });
        // A name the binding binds reads as the closed type it binds; any other is gathered.
        let mut closed = BumpVec::new_in(scratch);
        for (name, index) in map.iter() {
            let level = (born.levels.get(index)).map(|at| judge::own_variable(types, *at));
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
        // Each bound level is the point its binding gives it: a substitution, which commutes.
        Some(judge::read_points(
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
        match self.callable_cell(shape) {
            typed @ Static::Rigid { variables, .. } if typed.over_paths() => {
                self.through_paths(read, variables)
            }
            _ => Some(read),
        }
    }

    /// The solution the callable body `body` is born instantiated at, where the walk fixed one.
    pub(super) fn born_instance(&self, body: &BodyShape<'graph>) -> Option<StaticInstance<'o>> {
        self.born(body.segment())?.instance
    }

    /// Fix the solution the callable body `body` is born instantiated at, in the unit birthing it.
    pub(super) fn fix_born_instance(
        &mut self,
        body: &'graph BodyShape<'graph>,
        solution: StaticInstance<'o>,
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
        let kept = match self.level_of(body.segment()) {
            Some(level) => self.chain[level].signature,
            None => self.inline_facts(body.segment(), |facts| facts.signature)?,
        };
        judge::static_of(self.types, kept?)
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
            self.walked.contains(&segment) || self.store.wakes(frontier, self.types) != Wake::Open
        })
    }

    /// Whether the walk has walked `nested`, or need not: a quote's code the shape refused is left.
    pub(super) fn done(&self, nested: &BodyShape<'graph>) -> bool {
        self.walked.contains(&nested.segment())
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
        let types = self.types;
        // A signature, an application of one, or a variable bounded by either; only a variable's
        // own node is read as a variable, so a type built over one is no module.
        let node = judge::module_node(self, types, typed.upper)?;
        let signature = match node {
            // A signature is closed content: no substitution enters one.
            TypeNode::Signature { .. } => judge::static_of(types, typed.upper)?,
            TypeNode::SignatureApply { signature, .. } => signature,
            // A meet is a module binder's where each member is. A meet over a variable is the meet
            // rule's name, so no member is one.
            TypeNode::SignatureMeet { members } => {
                for member in members.iter() {
                    let signature = match judge::node_unless_variable(types, member)? {
                        TypeNode::Signature { .. } => judge::static_of(types, member)?,
                        TypeNode::SignatureApply { signature, .. } => signature,
                        _ => return None,
                    };
                    schema_of(signature, types)?;
                }
                return Some(Unknown {
                    segment: shape.segment(),
                    at: Address::Slot(slot),
                });
            }
            _ => {
                let bound = node.rigid_bound()?;
                match judge::closed_type_node(types, bound) {
                    TypeNode::Signature { .. } => bound,
                    TypeNode::SignatureApply { signature, .. } => signature,
                    _ => return None,
                }
            }
        };
        schema_of(signature, types)?;
        Some(Unknown {
            segment: shape.segment(),
            at: Address::Slot(slot),
        })
    }

    /// Gather the unknown the binder at `slot` of the shape at `level`, of static type `bound`,
    /// is itself, where it is one: a module binder, whose every read is of its content. A type
    /// name's seed asks nothing: it is the same at load and under every binding.
    pub(super) fn note_own(&self, level: usize, slot: Slot, bound: Bound, tree: Option<Tree<'o>>) {
        debug_assert!(!self.standing(level), "a standing level seeds nothing");
        let shape = self.chain[level].shape;
        let homed = (self.shapes.level(shape.segment(), Address::Slot(slot))).is_some();
        let closed = matches!(
            self.chain[level].declared[slot.index()],
            Some((Static::Closed(_), _))
        );
        // A type name's seed is the decided name of its kind's type value, the same at load and
        // under every binding ([`kind_value`](crate::elaborate::judge::kind_value)). The chain
        // homes type names alone.
        if homed && !closed {
            assert!(
                matches!(shape.slot_name(slot), BinderSymbol::Type(_)),
                "a homed value name"
            );
            return;
        }
        if let Some(own) = self.module_unknown(level, slot, bound) {
            self.note_module(own, bound, tree);
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
        layout
            .get(index)
            .is_some_and(|(level, _)| self.composed.contains(&(body.segment(), *level)))
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> Marks
    for Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Note open each run-bound name of the chain `judged` or `slot` holds as a lexical variable.
    /// A path variable's module binder is open already: every read of a module binder's content
    /// marks it ([`ModuleContent`](OpenKind::ModuleContent)).
    fn open(
        &self,
        kind: OpenKind,
        slot: Option<DeclaredType<Parametric>>,
        judged: DeclaredType<Parametric>,
    ) {
        let mark = OpenMark::undecided(kind, slot, judged);
        let (types, scratch) = (self.types, self.scratch);
        let Some(innermost) = self.chain.last() else {
            return;
        };
        let segment = innermost.shape.segment();
        for ty in std::iter::once(judged).chain(slot) {
            for variable in free_variables(types, scratch, ty) {
                self.each_unknown_of(segment, variable, &mut |unknown| {
                    self.note_open(unknown, mark);
                });
            }
        }
    }

    /// Ask the store for its witness's answer — the one call of [`Store::answer`] — and note it
    /// beside the question on each run-bound name of the chain the question's types hold.
    fn witnessed(&self, question: Question<'_>) -> Option<Answer> {
        let (types, scratch) = (self.types, self.scratch);
        let unknown_of = |level| self.unknown_of_level(level);
        let answered = self.store.answer(question, &unknown_of, types, scratch)?;
        let mut judged = None;
        question.each_type(&mut |ty| _ = judged.get_or_insert(ty));
        let mark = OpenMark {
            kind: question.kind(),
            slot: None,
            judged: judged.expect("a question judges a type"),
            answered: Some(answered),
        };
        let mut noted = BumpVec::new_in(scratch);
        let segment = (self.chain.last()).map(|innermost| innermost.shape.segment());
        question.each_type(&mut |ty| {
            for variable in free_variables(types, scratch, ty) {
                if let Some(segment) = segment {
                    self.each_unknown_of(segment, variable, &mut |unknown| {
                        if !noted.contains(&unknown) {
                            noted.push(unknown);
                        }
                    });
                }
            }
        });
        noted
            .iter()
            .for_each(|unknown| self.note_open(*unknown, mark));
        #[cfg(test)]
        super::QUESTIONS.with(|asked| asked.set(asked.get() + 1));
        Some(answered.answer)
    }

    /// The name for `term`: every walk names a term alike, and the store lays it where no layer
    /// beneath holds it. A term whose ends hold an opaque carrier has none, as a bound holds no
    /// carrier, so its site keeps the type it gives outside a walk.
    fn derive(&self, term: DerivedTerm<'_>) -> Option<Parametric> {
        let (lower, bound) = term.ends(self.types, self.scratch);
        if self.types.holds_carrier(lower) || self.types.holds_carrier(bound) {
            return None;
        }
        Some(self.store.derived(self.types, term).ty)
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        self.store.derived_name(id).map(|name| name.term)
    }
}

/// What the walk keeps of reading the records its binding names but does not wake, closed
/// ([`Wake::Named`]): each type closed, by the segment whose levels it is read at; each record
/// closed, by where it rests; and each type
/// closed whole beside its closed type — the entries of the layer's table of closed types, laid
/// where the walk ends.
pub(super) struct Rebound<'p, 'o> {
    pub(super) types: BumpBackedMap<'p, (Segment, DeclaredType<Parametric>), Closed>,
    pub(super) units: BumpBackedMap<'p, usize, &'o UnitFacts<'o>>,
    pub(super) closings: BumpBackedMap<'p, usize, &'o Closing<'o>>,
    pub(super) table: BumpBackedMap<'p, DeclaredType<Parametric>, Closed>,
    /// Each open tree an absorbed record holds that the binding changes, by identity, beside the
    /// tree a fresh load builds for it: the entries of the layer's table of trees.
    pub(super) trees: BumpBackedMap<'p, Identity, &'o Tree<'o>>,
    /// Each class the binding moves a record's paths to, beside what it reads: found as a class
    /// the walk resolved is, though no record holds it.
    pub(super) classes:
        BumpBackedMap<'p, crate::type_lattice::PathClass, super::paths::ClassOf<'o>>,
}

/// The walk's closer for a record of `segment`'s: substitution under the walk's binding, each
/// level read from `segment` ([`Pass::rebind`]), laid through the walk's lay.
struct Rebinding<'c, P> {
    pass: &'c P,
    segment: Segment,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> Closer
    for Rebinding<'_, Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>>
{
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        self.pass.rebind(self.segment, ty)
    }

    fn class(&mut self, class: crate::type_lattice::PathClass) -> Option<crate::scope::Resolved> {
        self.pass.class_bound(self.segment, class)
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> crate::scope::Laying<'o>
    for Rebinding<'_, Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>>
{
    type Lay = S::Lay;

    fn lay(&self) -> S::Lay {
        self.pass.lay
    }

    fn tree(&mut self, identity: Identity) -> Option<&'o Tree<'o>> {
        self.pass.rebound.borrow().trees.get(&identity).copied()
    }
}

/// The walk's binding as a substitution reads it from `segment`: each level the binding gives a
/// type, each derived name's term from the tables the store reads, and a term no table holds named
/// and laid by the store.
struct Rebinds<'c, P> {
    pass: &'c P,
    segment: Segment,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> crate::scope::Binds
    for Rebinds<'_, Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>>
{
    fn level(&self, _: &crate::values::Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        self.pass.level_bound(self.segment, level)
    }

    fn term(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedTerm<'_>> {
        self.pass.store.derived_name(id).map(|name| name.term)
    }

    fn class(
        &self,
        _: &crate::values::Chain<'_, '_, '_, '_>,
        _: crate::memory::BumpAllocator<'_>,
        class: crate::type_lattice::PathClass,
    ) -> Option<crate::scope::Resolved> {
        self.pass.class_bound(self.segment, class)
    }

    fn name(
        &self,
        types: &crate::values::Chain<'_, '_, '_, '_>,
        _: crate::memory::BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        Some(self.pass.store.derived(types, term).ty)
    }

    fn carried(&self, binder: Unknown) -> Option<KType> {
        match self.pass.bound_through(binder)? {
            crate::scope::Bound::Module { carried, .. } => Some(carried),
            crate::scope::Bound::Type(_) => None,
        }
    }
}

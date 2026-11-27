//! The walk's **façade**: the one door every read and write of a fact goes through, so a rule
//! never names where a fact rests ([the store](../../build/README.md#the-store)).
//!
//! A fact the walk fixed rests in its shape's level while the shape is on the chain, beside the
//! unit that fixed it, and once it leaves the chain in that unit's record, an inline body's as an
//! owning body's; a module body's signature in the record of the unit that ran it; a callable's or
//! a quote's code's birth facts in its birth unit's; a body's closing facts in its closing record.
//!
//! A read gathers what the fact waits on, at unit grain: nothing for a fact of the unit running,
//! the gathering so far of a unit still running around it, and a read of the frontier of one that
//! ended or was only read, cut to the chain. An entry unit seeds each binder in a gathering of its
//! own, its **seed**, and a read of a binder gathers its seed alone. Only an unknown of a shape on
//! the chain is gathered. A record notes each variable its types name
//! ([`note_named`](Pass::note_named)); a door notes what it judged open.
//!
//! The walk binds nothing: every unit it reaches runs, and a name it reads open stays open, for
//! the binding graph to answer the questions laid over it.

use crate::elaborate::judge::{self, Derives, Marks, free_variables};
use crate::elaborate::schema_of;
use crate::memory::BumpVec;
use crate::memory::Lays;
use crate::scope::KeyedEntry;
use crate::scope::{
    Address, Answer, Arms, BinderAt, BodyShape, Born, Cut, DerivedId, DerivedTerm, Frontier,
    Holder, Identity, InstanceRead, Mark, Narrowing, Noted, OpenKind, OpenMark, Pick, Points,
    Posed, Question, Restamp, Route, Segment, ShapeKind, Site, SiteQuestion, Slot, Static,
    StaticCallable, StaticInstance, StaticRegistered, StaticType, Store, Traced, Tree, Typed,
    UnitFacts, UnitId, Unknown, UseStatics, in_body_of,
};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Record, Side, TypeNode, Variable,
};

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

    /// Whether `unknown` is gathered: of a shape on the chain.
    fn gathers(&self, unknown: Unknown) -> bool {
        (self.chain.iter()).any(|level| level.shape.segment() == unknown.segment)
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
    pub(super) fn note_open(&self, unknown: Unknown, mark: OpenMark) {
        self.noting(std::iter::once(Noted {
            unknown,
            mark: Mark::Open(mark),
        }));
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

    /// How far the walk has gathered: how many gatherings are open, and how much the innermost
    /// holds. What [`ungather`](Self::ungather) drops back to.
    pub(super) fn gathered(&self) -> (usize, [usize; 3]) {
        let scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let held = (scopes.last()).map_or([0; 3], |open| {
            [open.noted.len(), open.reads.len(), open.posed.len()]
        });
        let gathered = (scopes.len(), held);
        self.scopes.set(scopes);
        gathered
    }

    /// Drop what was gathered since `gathered`, so an attempt typed again gathers it once.
    pub(super) fn ungather(&self, (depth, [noted, reads, posed]): (usize, [usize; 3])) {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        scopes.truncate(depth);
        if let Some(open) = scopes.last_mut() {
            open.noted.truncate(noted);
            open.reads.truncate(reads);
            open.posed.truncate(posed);
        }
        self.scopes.set(scopes);
    }

    /// The questions the innermost gathering's unit posed, laid through `lay` in order.
    pub(super) fn posed<'to>(&self, lay: impl Lays<'to>) -> &'to [Posed<'to>] {
        let scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let posed = (scopes.last()).map_or(&[][..], |open| {
            lay.fill(open.posed.len(), |at| open.posed[at].laid(lay))
        });
        self.scopes.set(scopes);
        posed
    }

    /// Take the questions the innermost gathering's unit posed, leaving it none.
    pub(super) fn take_posed(&self) -> BumpVec<'p, Posed<'p>> {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        let posed = (scopes.last_mut())
            .map(|open| std::mem::replace(&mut open.posed, BumpVec::new_in(self.scratch)));
        self.scopes.set(scopes);
        posed.unwrap_or_else(|| BumpVec::new_in(self.scratch))
    }

    /// Add `posed` to the innermost gathering's questions: a seed's are its entry unit's.
    pub(super) fn repose(&self, posed: BumpVec<'p, Posed<'p>>) {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            open.posed.extend(posed);
        }
        self.scopes.set(scopes);
    }

    /// Add each of `noted` the walk gathers to the innermost gathering.
    fn noting(&self, noted: impl Iterator<Item = Noted>) {
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
    /// innermost gathering, cut to the chain: one entry however many unknowns it holds.
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

    /// The record of `unit` the store holds, beside the tag a read of one of its facts gathers:
    /// one per unit, made at its first read. For a unit the walk does not run.
    pub(super) fn read_unit(&self, unit: UnitId) -> Option<(Tag, &'o UnitFacts<'o>)> {
        let (facts, frontier) = self.store.fact(unit)?;
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

    /// The state of `unit` in this walk, where it ran it.
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
    pub(super) fn binder_at(&self, level: usize, slot: Slot) -> Bound<'o> {
        self.binder_holding(level, slot, None)
    }

    /// [`binder_at`](Self::binder_at) for a binder whose content tree the reader holds as `tree`,
    /// where the walk may not have fixed it yet.
    pub(super) fn binder_holding(
        &self,
        level: usize,
        slot: Slot,
        tree: Option<Tree<'o>>,
    ) -> Bound<'o> {
        let (bound, tag) = self.binder_held(level, slot);
        if let Some(tag) = tag {
            self.gather(tag);
        }
        if let Some(unknown) = self.module_unknown(level, slot, bound) {
            self.note_module(unknown, bound, tree);
        }
        bound
    }

    /// The static type of the part held in place at `site` of the shape at `level`, its content
    /// tree `tree`, gathered as [`binder_holding`](Self::binder_holding) gathers a binder's:
    /// `[Never, Any]` where the walk holds no part there.
    pub(super) fn held_holding(&self, level: usize, site: Site, tree: Tree<'o>) -> Bound<'o> {
        let at = &self.chain[level];
        let held = (at.held.iter().any(|(each, _, _)| *each == site))
            .then(|| at.parts.get(&site).copied())
            .flatten();
        let Some((typed, tag)) = held else {
            return Bound::Type(crate::values::unknown());
        };
        self.gather(tag);
        let binder = crate::scope::Binder {
            body: at.shape.segment(),
            at: BinderAt::Held(site),
        };
        if self.holds_module(typed) {
            self.note_module(binder.unknown(), Bound::Type(typed), Some(tree));
        }
        Bound::Type(typed)
    }

    /// Gather the module binder `unknown`, of static type `bound`, its content tree `tree` where
    /// the reader holds it: named where `bound` is exact over its own paths, or the type it carries,
    /// alone, which a binding of it closes to the type its retype stamps, the type it binds to;
    /// else as a read of its content, by the names it reads through.
    fn note_module(&self, unknown: Unknown, bound: Bound<'o>, tree: Option<Tree<'o>>) {
        let own = match bound {
            Bound::Type(typed) => self.exact_over_own(unknown, typed),
            Bound::Scheme(..) => false,
        };
        if own {
            return self.note_named_unknown(unknown);
        }
        // A local whose content the load built closed reads no name. One whose tree a binding of
        // the names its leaves read gives whole reads those names; any other reads its own,
        // whose binding gives the tree whole, beside them.
        match tree.or_else(|| self.local_content(unknown)) {
            Some(tree) if tree.closed() && !matches!(tree, Tree::Leaf(_)) => {}
            Some(tree) if self.named_through(tree) => self.note_named_of(tree),
            Some(tree) => {
                self.note_named_unknown(unknown);
                self.note_named_of(tree);
            }
            None => self.note_named_unknown(unknown),
        }
    }

    /// Whether `typed`, the static type of the module binder `unknown`, is one point written over
    /// its own paths, or the type it carries, alone: a binding of it closes such a type to the type
    /// it binds.
    pub(super) fn exact_over_own(&self, unknown: Unknown, typed: Interval) -> bool {
        if typed.lower != typed.upper {
            return false;
        }
        let upper = DeclaredType::Type(typed.upper);
        (free_variables(self.types, self.scratch, upper).iter()).all(|variable| match *variable {
            Variable::Path { class, .. } => self.class_binder(class) == Some(unknown),
            Variable::Derived { term, .. } => matches!(self.term(term),
                Some(DerivedTerm::Carried { binder, .. }) if binder == unknown),
            _ => false,
        })
    }

    /// Whether `typed`, the static type of the module binder `unknown` whose content tree is
    /// `tree`, is one point written over names a binding closes as a fresh load with that binding
    /// declared types the binder: type names, and of the module binders, the binder, a holder of
    /// its tree, or a name its tree reads through.
    pub(super) fn exact_over_read(
        &self,
        unknown: Unknown,
        typed: Interval,
        tree: Tree<'o>,
    ) -> bool {
        if typed.lower != typed.upper {
            return false;
        }
        let mut read = BumpVec::new_in(self.scratch);
        read.push(unknown);
        self.leaf_names(tree, &mut |name| read.push(name));
        // Every holder of the binder's tree holds its content: a binding of one gives the others.
        self.holders_of(unknown.segment, tree.identity(), &mut |holder| {
            read.push(holder.unknown());
            false
        });
        let upper = DeclaredType::Type(typed.upper);
        (free_variables(self.types, self.scratch, upper).iter()).all(|variable| {
            let mut names = 0;
            let mut within = true;
            self.each_unknown_of(unknown.segment, *variable, &mut |name| {
                names += 1;
                within &= read.contains(&name) || !self.is_module_binder(name);
            });
            names > 0 && within
        })
    }

    /// The content tree the walk holds for `unknown`, a module binder no parameter: the newest a
    /// unit of a level the walk runs fixed, else the one its own unit read off its right-hand side
    /// before fixing it.
    fn local_content(&self, unknown: Unknown) -> Option<Tree<'o>> {
        let Address::Slot(slot) = unknown.at else {
            return None;
        };
        let level = self.level_of(unknown.segment)?;
        if self.chain[level].shape.position_of(slot) == crate::scope::Position::PARAMETER {
            return None;
        }
        (self.holding.get(&(unknown.segment, Holder::Slot(slot))))
            .map(|(tree, _, _)| *tree)
            .or_else(|| {
                let shape = super::address(self.chain[level].shape);
                (self.binders.get(&(shape, slot, None))).map(|(tree, _)| *tree)
            })
    }

    /// Gather `binder`, of the shape at `level`, named where a content tree holds a leaf on it: a
    /// module binder whose content a binding gives the tree by tree substitution.
    pub(super) fn note_leaf(&self, level: usize, binder: crate::scope::Binder) {
        if let Some(unknown) = self.chain_module_at(level, binder) {
            self.note_named_unknown(unknown);
        }
    }

    /// Gather named each name [`leaf_names`](Self::leaf_names) hands out for `tree` that the binding
    /// leaves unbound: a binding of it substitutes the tree.
    pub(super) fn note_named_of(&self, tree: Tree<'o>) {
        let mut named = BumpVec::new_in(self.scratch);
        self.leaf_names(tree, &mut |unknown| {
            if !named.contains(&unknown) {
                named.push(unknown);
            }
        });
        self.noting(named.into_iter().map(Noted::named));
    }

    /// Hand `each` the names a read of `tree`, a module's content tree, reads through: each module
    /// binder of the chain a leaf is on, and what decides each call leaf
    /// ([`call_unknowns`](Self::call_unknowns)). Nothing for a closed tree.
    pub(super) fn leaf_names(&self, tree: Tree<'o>, each: &mut dyn FnMut(Unknown)) {
        for binder in self.leaf_binders(tree) {
            if let Some(unknown) = self.chain_module(*binder) {
                each(unknown);
            }
        }
        self.call_unknowns(&tree, each);
    }

    /// Whether `unknown` is a module binder of the chain.
    fn is_module_binder(&self, unknown: Unknown) -> bool {
        let Address::Slot(slot) = unknown.at else {
            return false;
        };
        let binder = crate::scope::Binder::at_slot(unknown.segment, slot);
        self.chain_module(binder).is_some()
    }

    /// The unknown of `binder`, where it is a module binder of the chain.
    pub(super) fn chain_module(&self, binder: crate::scope::Binder) -> Option<Unknown> {
        self.chain_module_at(self.level_of(binder.body)?, binder)
    }

    /// The unknown of `binder`, of the shape at `level`, where it is a module binder.
    fn chain_module_at(&self, level: usize, binder: crate::scope::Binder) -> Option<Unknown> {
        match binder.at {
            BinderAt::Slot(slot) => {
                let (bound, _) = self.binder_held(level, slot);
                self.module_unknown(level, slot, bound)
            }
            BinderAt::Held(site) => {
                let typed = self.held_type(level, site)?;
                self.holds_module(typed).then_some(binder.unknown())
            }
        }
    }

    /// Whether a binding of the names [`leaf_names`](Self::leaf_names) hands out gives `tree`'s
    /// content whole: an open tree reading some such name, each of whose leaves is a call or on a
    /// module binder of the chain, read in the run being typed.
    pub(super) fn named_through(&self, tree: Tree<'o>) -> bool {
        let mut names = false;
        self.leaf_names(tree, &mut |_| names = true);
        matches!(tree.identity(), Identity::Open(_)) && names && self.leaves_named(tree)
    }

    /// Whether every leaf of `tree` is one [`leaf_names`](Self::leaf_names) names.
    fn leaves_named(&self, tree: Tree<'o>) -> bool {
        use crate::scope::Leaf;
        if tree.closed() {
            return true;
        }
        let own = match tree {
            Tree::Leaf(Leaf::Bound {
                binder,
                within: None,
                ..
            }) => self.chain_module(binder).is_some(),
            Tree::Leaf(
                Leaf::Bound {
                    within: Some(_), ..
                }
                | Leaf::Unnamed(_),
            ) => false,
            _ => true,
        };
        let mut named = own;
        tree.subtrees(&mut |subtree| named = named && self.leaves_named(*subtree));
        named
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
    pub(super) fn binder_fact(&self, level: usize, slot: Slot) -> Bound<'o> {
        self.binder_held(level, slot).0
    }

    /// The static type of the binder at `slot` of the shape at `level`, beside the unit that fixed
    /// it: `[Never, Any]` where none has.
    fn binder_held(&self, level: usize, slot: Slot) -> (Bound<'o>, Option<Tag>) {
        self.chain[level].binders[slot.index()]
    }

    /// Fix the static type of the binder at `slot` of the shape at `level`.
    pub(super) fn set_binder(&mut self, level: usize, slot: Slot, bound: Bound<'o>) {
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
        let last = self.chain[level].shape.body().len().checked_sub(1)?;
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
        let (typed, tag) = self.part_held(level, site)?;
        self.gather(tag);
        Some(typed)
    }

    /// The static type the walk recorded for the part at `site` of the shape at `level`, beside the
    /// unit that recorded it, read without gathering it.
    pub(super) fn part_held(&self, level: usize, site: Site) -> Option<(Interval, Tag)> {
        self.chain[level].parts.get(&site).copied()
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

    /// Record what an answer selects among at the keyworded use at `index` of the shape at
    /// `level`.
    pub(super) fn set_undecided(
        &mut self,
        level: usize,
        index: usize,
        undecided: &'o crate::scope::Selection<'o>,
    ) {
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
        let tag = self.innermost();
        let at = &mut self.chain[level].uses[index];
        at.0.contributions = contributions;
        at.1 = Some(tag);
        self.wrote(level, tag, Wrote::Use(index));
    }

    /// The point each variable of the candidate selected at the use at `site` of the shape at
    /// `level` solves to, where the walk selected one there.
    pub(super) fn selection_at(&self, level: usize, site: Site) -> Option<Points<'o>> {
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
        let tag = self.innermost();
        self.chain[level].selections.insert(site, (points, tag));
        self.wrote(level, tag, Wrote::Selection(site));
    }

    /// Record what the selected use at `site` fixes of its callee's names.
    pub(super) fn record_traced(&mut self, level: usize, site: Site, traced: Traced<'o>) {
        let tag = self.innermost();
        self.chain[level].traced.insert(site, (traced, tag));
        self.wrote(level, tag, Wrote::Traced(site));
    }

    /// Record the content tree of the view the `:|` at `site` builds.
    pub(super) fn record_view_tree(&mut self, level: usize, site: Site, tree: &'o Tree<'o>) {
        let tag = self.innermost();
        let index = self.chain[level].trees.len();
        self.chain[level].trees.push((site, tree, tag));
        self.wrote(level, tag, Wrote::Tree(index));
        self.hold_view(self.chain[level].shape.segment(), site, tree, tag);
    }

    /// Record the picks of the view the `:|` at `site` builds.
    pub(super) fn record_picks(&mut self, level: usize, site: Site, picks: &'o [Pick<'o>]) {
        let tag = self.innermost();
        let index = self.chain[level].picks.len();
        self.chain[level].picks.push((site, picks, tag));
        self.wrote(level, tag, Wrote::Picks(index));
    }

    /// Record the content tree of the module binder at `slot` of the shape at `level`.
    pub(super) fn record_content(&mut self, level: usize, slot: Slot, tree: Tree<'o>) {
        let tag = self.innermost();
        let identities = self.held_identities(tree);
        let index = self.chain[level].contents.len();
        self.chain[level].contents.push((slot, tree, tag));
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

    /// Record a part of the shape at `level` held in place at `site`, its content tree `tree`:
    /// a holder of each module the tree holds, as a module binder is.
    pub(super) fn record_held(&mut self, level: usize, site: Site, tree: Tree<'o>) {
        let tag = self.innermost();
        let identities = self.held_identities(tree);
        let index = self.chain[level].held.len();
        self.chain[level].held.push((site, tree, tag));
        self.wrote(level, tag, Wrote::Held(index));
        self.hold(
            self.chain[level].shape.segment(),
            Holder::Held(site),
            tree,
            identities,
            tag,
        );
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

    /// Lay `question`, a rule decided at a site, in the record of the unit writing there: one per
    /// site and rule, the last laid standing.
    pub(super) fn lay_question(&mut self, question: SiteQuestion<'o>) {
        let owner = self.owner_of(self.innermost());
        let questions = &mut self.units[owner.0 as usize].questions;
        questions.retain(|held| held.key() != question.key());
        questions.push(question);
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

    /// Record what the retype at `site` re-stamps at.
    pub(super) fn record_retype(&mut self, level: usize, site: Site, retype: Restamp) {
        let tag = self.innermost();
        let index = self.chain[level].retypes.len();
        self.chain[level].retypes.push((site, retype, tag));
        self.wrote(level, tag, Wrote::Retype(index));
    }

    /// Record what the load fixed about the arms of the `MATCH` whose node sits at `site`.
    pub(super) fn record_arms(&mut self, level: usize, site: Site, arms: Arms<'o>) {
        let tag = self.innermost();
        let index = self.chain[level].arms.len();
        self.chain[level].arms.push((site, arms, tag));
        self.wrote(level, tag, Wrote::Arms(index));
    }

    /// Record the route of the tail call whose node sits at `site`.
    pub(super) fn record_route(&mut self, level: usize, site: Site, route: Route<'o>) {
        let tag = self.innermost();
        let index = self.chain[level].routes.len();
        self.chain[level].routes.push((site, route, tag));
        self.wrote(level, tag, Wrote::Route(index));
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
        let typed = statement.and_then(|statement| {
            let unit = self.shapes.running(segment, statement);
            self.owner_facts(segment, unit, read).flatten()
        });
        typed.unwrap_or(Static::Unknown)
    }

    /// Fix the type of the type expression at `site` of the shape at `level`.
    pub(super) fn set_expression(&mut self, level: usize, site: Site, typed: StaticType<'o>) {
        let tag = self.innermost();
        self.chain[level].expressions.insert(site, (typed, tag));
        self.wrote(level, tag, Wrote::Expression(site));
    }

    /// The declaration the walk fixed for the type binder at `slot` of `shape`, and the binder
    /// itself where it is unknown.
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
        // A bound over an encloser's names is read at its upper end, as its own levels are.
        let callable = born.callable;
        let map = match callable {
            Static::Closed(callable) => callable.quantifier_map,
            Static::Rigid { value, .. } => value.quantifier_map,
            Static::Unknown => return None,
        };
        let (types, scratch) = (self.types, self.scratch);
        let (params, ret) = self.function_in_body(callable, born.levels)?;
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
        // Each of its own names the part reads is gathered.
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
                self.note(&[unknown]);
            }
        }
        Some(read)
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
    /// exactly: its laid signature closed, and no `Never`, which a member no run knows exactly
    /// gives.
    pub(super) fn signature_of(&self, body: &BodyShape<'graph>) -> Option<KType> {
        let kept = match self.level_of(body.segment()) {
            Some(level) => self.chain[level].signature,
            None => self.inline_facts(body.segment(), |facts| facts.signature)?,
        };
        judge::static_of(self.types, kept?).filter(|signature| *signature != KType::NEVER)
    }

    /// The last statement's static type of the quote's code `code` as it runs unfilled.
    pub(super) fn ran_of(&self, code: &BodyShape<'graph>) -> Option<Interval> {
        self.closed(code.segment())?.ran
    }

    /// Whether the closing record of `body` stands for this walk: one it wrote. A test of progress,
    /// which gathers nothing.
    pub(super) fn has_closed(&self, body: &BodyShape<'graph>) -> bool {
        let segment = body.segment();
        self.store.close(segment).is_some() && self.walked.contains(&segment)
    }

    /// Whether the walk has walked `nested`, or need not: a quote's code the shape refused is left.
    pub(super) fn done(&self, nested: &BodyShape<'graph>) -> bool {
        self.walked.contains(&nested.segment())
            || (nested.kind() == ShapeKind::Code && nested.refusal().is_some())
    }

    /// The unknown the binder at `slot` of the shape at `level` is, where it is a module binder:
    /// a binding binds it to a closed content tree, so every fact reading its content waits on it.
    pub(super) fn module_unknown(
        &self,
        level: usize,
        slot: Slot,
        bound: Bound<'o>,
    ) -> Option<Unknown> {
        let shape = self.chain[level].shape;
        let (BinderSymbol::Value(_), Bound::Type(typed)) = (shape.slot_name(slot), bound) else {
            return None;
        };
        self.holds_module(typed).then_some(Unknown {
            segment: shape.segment(),
            at: Address::Slot(slot),
        })
    }

    /// Whether a value binder of static type `typed` is a module binder: a signature, an
    /// application of one, a meet of those, or a variable bounded by either.
    fn holds_module(&self, typed: Interval) -> bool {
        let types = self.types;
        // Only a variable's own node is read as a variable, so a type built over one is no module.
        let Some(node) = judge::module_node(self, types, self.scratch, typed.upper) else {
            return false;
        };
        let signature = match node {
            // A signature is closed content: no substitution enters one.
            TypeNode::Signature { .. } => judge::static_of(types, typed.upper),
            TypeNode::SignatureApply { signature, .. } => Some(signature),
            // A meet is a module binder's where each member is. A meet over a variable is the meet
            // rule's name, so no member is one.
            TypeNode::SignatureMeet { members } => {
                return members.iter().all(|member| {
                    let signature = match judge::node_unless_variable(types, member) {
                        Some(TypeNode::Signature { .. }) => judge::static_of(types, member),
                        Some(TypeNode::SignatureApply { signature, .. }) => Some(signature),
                        _ => None,
                    };
                    signature
                        .and_then(|signature| schema_of(signature, types))
                        .is_some()
                });
            }
            _ => node
                .rigid_bound()
                .and_then(|bound| match judge::closed_type_node(types, bound) {
                    TypeNode::Signature { .. } => Some(bound),
                    TypeNode::SignatureApply { signature, .. } => Some(signature),
                    _ => None,
                }),
        };
        signature
            .and_then(|signature| schema_of(signature, types))
            .is_some()
    }

    /// Gather the unknown the binder at `slot` of the shape at `level`, of static type `bound`,
    /// is itself, where it is one: a module binder, whose every read is of its content. A type
    /// name's seed asks nothing: it is the same at load and under every binding.
    pub(super) fn note_own(
        &self,
        level: usize,
        slot: Slot,
        bound: Bound<'o>,
        tree: Option<Tree<'o>>,
    ) {
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
    /// Note open each run-bound name of the chain `judged` or `slot` is written over: a lexical
    /// variable's, and the names a path variable or a derived name reads through.
    fn open(
        &self,
        kind: OpenKind,
        slot: Option<DeclaredType<Parametric>>,
        judged: DeclaredType<Parametric>,
    ) {
        #[cfg(test)]
        self.check_posed(kind, slot, judged);
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

    /// Note `question` in the innermost gathering, laid in the walk's scratch, beside `rigid`.
    fn asked(&self, question: Question<'_>, rigid: Option<Answer>) {
        let mut scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        if let Some(open) = scopes.last_mut() {
            let question = question.laid(self.scratch);
            open.posed.push(Posed { question, rigid });
        }
        self.scopes.set(scopes);
        #[cfg(test)]
        super::count(|counts| counts.posed += 1);
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        self.store.derived_name(id).map(|name| name.term)
    }

    fn declares_type(&self, level: usize) -> bool {
        (self.chain.last()).is_some_and(|at| self.shapes.declares_type(at.shape.segment(), level))
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> Derives
    for Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    type Unwalked = std::convert::Infallible;

    /// The name for `term`: every walk names a term alike, and the store lays it where no layer
    /// beneath holds it.
    fn derive(&self, term: DerivedTerm<'_>) -> Result<Parametric, std::convert::Infallible> {
        Ok(self.store.derived(self.types, term).ty)
    }
}

#[cfg(test)]
impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Refuse an open mark over `judged` and `slot` that the innermost gathering's last undecided
    /// posed question does not judge: a door that marks without posing its question whole. So
    /// every test's load checks it.
    fn check_posed(
        &self,
        kind: OpenKind,
        slot: Option<DeclaredType<Parametric>>,
        judged: DeclaredType<Parametric>,
    ) {
        let scopes = self.scopes.replace(BumpVec::new_in(self.scratch));
        // Marking reads a name's unknowns, which may pose rigid questions of its own.
        let posed = (scopes.last()).map(|open| {
            (open.posed.iter().rev())
                .find(|posed| posed.rigid.is_none())
                .copied()
        });
        self.scopes.set(scopes);
        let Some(posed) = posed else {
            return;
        };
        let judges = |posed: Posed<'_>| {
            let mut held = BumpVec::new_in(self.scratch);
            posed.question.each_type(&mut |ty| held.push(ty));
            held.contains(&judged) && slot.is_none_or(|slot| held.contains(&slot))
        };
        assert!(
            posed.is_some_and(judges),
            "a door marks {kind:?} open without posing its question whole"
        );
    }
}

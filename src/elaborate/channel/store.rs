//! The type channel's **fact store**: the one door every read and write of a channel cell goes
//! through ([the build module's design](../../build/README.md#type-facts-and-the-solver)). The
//! walk reaches a type binder's, a registration's, a type expression's or a callable's cell only
//! through the methods here, which `tests::boundary` holds it to, so the store is the one place a
//! read is recorded.
//!
//! In a resumption the store reads each cell as the resumption has it — re-run by this walk, else
//! as the resumptions it extends settled it, else the load's — and writes each cell it re-runs
//! beside the load's, never into it ([`Resume`]).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Writer, bump_table, collect};
use crate::scope::{
    BodyShape, ChannelSettled, FactKey, Keyed, PathEntry, PathRead, Residual, Resumption, Segment,
    Settled, ShapeError, Site, Slot, StaticCallable, StaticRegistered, StaticType, Target,
    TypeExpression, in_body_of,
};
use crate::type_lattice::Parametric;

use super::{Pass, edit};

/// A cell a resumption re-ran.
#[derive(Clone, Copy)]
enum Rerun<'graph> {
    Declared(StaticType<'graph>),
    Registered(StaticRegistered<'graph>),
    Expression(StaticType<'graph>),
    Callable(StaticCallable<'graph>),
}

/// A resumption the walk runs: what it resumes under, and each cell it re-ran, by its shape's
/// segment and where the cell lies.
pub(super) struct Resume<'p, 'graph> {
    pub(super) over: Resumption<'p, 'graph>,
    cells: Cell<BumpBackedMap<'p, (Segment, FactKey), Rerun<'graph>>>,
    levels: Cell<BumpVec<'p, (Segment, &'graph [Parametric])>>,
    residuals: Cell<BumpVec<'p, (Segment, Residual<'graph>)>>,
    refusals: Cell<BumpVec<'p, (Segment, &'graph ShapeError<'graph>)>>,
    /// How many cells the walk re-ran.
    pub(super) reran: Cell<u32>,
    scratch: BumpAllocator<'p>,
}

impl<'p, 'graph> Resume<'p, 'graph> {
    pub(super) fn new(over: Resumption<'p, 'graph>, scratch: BumpAllocator<'p>) -> Self {
        Resume {
            over,
            cells: Cell::new(bump_table(scratch)),
            levels: Cell::new(BumpVec::new_in(scratch)),
            residuals: Cell::new(BumpVec::new_in(scratch)),
            refusals: Cell::new(BumpVec::new_in(scratch)),
            reran: Cell::new(0),
            scratch,
        }
    }

    fn cell(&self, segment: Segment, key: FactKey) -> Option<Rerun<'graph>> {
        let cells = self.cells.replace(bump_table(self.scratch));
        let held = cells.get(&(segment, key)).copied();
        self.cells.set(cells);
        held
    }

    fn rerun(&self, segment: Segment, key: FactKey, cell: Rerun<'graph>) {
        let mut cells = self.cells.replace(bump_table(self.scratch));
        cells.insert((segment, key), cell);
        self.cells.set(cells);
    }

    fn found<T: Copy>(
        held: &Cell<BumpVec<'p, (Segment, T)>>,
        scratch: BumpAllocator<'p>,
        segment: Segment,
    ) -> Option<T> {
        edit(held, scratch, |held| {
            (held.iter().rev())
                .find(|(at, _)| *at == segment)
                .map(|(_, found)| *found)
        })
    }

    /// What the walk settled, laid down: each shape's cells it re-ran, by segment.
    pub(super) fn settled(self, writer: Writer<'graph, 'graph>) -> &'graph Settled<'graph> {
        let scratch = self.scratch;
        let cells = self.cells.into_inner();
        let mut segments = BumpVec::new_in(scratch);
        segments.extend(cells.keys().map(|(segment, _)| *segment));
        let (levels, residuals, refusals) = (
            self.levels.into_inner(),
            self.residuals.into_inner(),
            self.refusals.into_inner(),
        );
        segments.extend(levels.iter().map(|(segment, _)| *segment));
        segments.extend(residuals.iter().map(|(segment, _)| *segment));
        segments.extend(refusals.iter().map(|(segment, _)| *segment));
        segments.sort_unstable();
        segments.dedup();
        let mut settled = BumpVec::with_capacity_in(segments.len(), scratch);
        for segment in segments.iter().copied() {
            let (mut declared, mut registered, mut expressions) = (
                BumpVec::new_in(scratch),
                BumpVec::new_in(scratch),
                BumpVec::new_in(scratch),
            );
            let mut callable = None;
            for ((at, key), cell) in cells.iter() {
                match (*at == segment, *key, *cell) {
                    (true, FactKey::Slot(slot), Rerun::Declared(typed)) => {
                        declared.push((slot, typed))
                    }
                    (true, FactKey::Slot(slot), Rerun::Registered(typed)) => {
                        registered.push((slot, typed))
                    }
                    (true, FactKey::Site(site), Rerun::Expression(typed)) => {
                        expressions.push((site, typed))
                    }
                    (true, FactKey::Shape, Rerun::Callable(typed)) => callable = Some(typed),
                    _ => {}
                }
            }
            fn last<T: Copy>(held: &[(Segment, T)], segment: Segment) -> Option<T> {
                (held.iter().rev())
                    .find(|(at, _)| *at == segment)
                    .map(|(_, found)| *found)
            }
            settled.push((
                segment,
                ChannelSettled {
                    declared: Keyed::sorted(writer, &mut declared),
                    registered: Keyed::sorted(writer, &mut registered),
                    expressions: Keyed::sorted(writer, &mut expressions),
                    callable,
                    levels: last(&levels, segment),
                    residual: last(&residuals, segment).unwrap_or(Keyed::from_sorted(&[])),
                    refusal: last(&refusals, segment),
                },
            ));
        }
        let mut laid = Settled::empty(self.over.binding);
        laid.channel = Keyed::from_sorted(collect(writer, settled.iter().copied()));
        laid.reran.0 = self.reran.get();
        crate::memory::resident(writer, laid)
    }
}

impl<'p, 'graph, X> Pass<'p, 'graph, '_, X> {
    /// The cell this resumption re-ran at `key` of `shape`, else the newest the resumptions it
    /// extends settled, read through `of`; `None` in the load, and where none re-ran it.
    fn settled_cell<T>(
        &self,
        shape: &BodyShape<'graph>,
        key: FactKey,
        own: impl Fn(Rerun<'graph>) -> Option<T>,
        of: impl Fn(&ChannelSettled<'graph>) -> Option<T>,
    ) -> Option<T> {
        let resume = self.resume.as_ref()?;
        if let Some(found) = resume.cell(shape.segment(), key).and_then(own) {
            return Some(found);
        }
        resume.over.channel(shape.segment()).find_map(of)
    }

    /// The cell of the type binder at `slot` of `shape`.
    pub(super) fn declared_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        let own = |cell| match cell {
            Rerun::Declared(typed) => Some(typed),
            _ => None,
        };
        let of = |settled: &ChannelSettled<'graph>| settled.declared.get(slot).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Slot(slot), own, of))
            .unwrap_or_else(|| shape.declared_type(slot))
    }

    /// The cell of the type binder at `slot` of `shape` as the load fixed it, which numbers the
    /// chain's lexical variables alike at every resumption.
    pub(super) fn loaded_declared(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        shape.declared_type(slot)
    }

    /// The cell of the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'graph> {
        let own = |cell| match cell {
            Rerun::Registered(typed) => Some(typed),
            _ => None,
        };
        let of = |settled: &ChannelSettled<'graph>| settled.registered.get(slot).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Slot(slot), own, of))
            .unwrap_or_else(|| shape.registered_type(slot))
    }

    /// The cell of the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'graph> {
        let own = |cell| match cell {
            Rerun::Expression(typed) => Some(typed),
            _ => None,
        };
        let of =
            |settled: &ChannelSettled<'graph>| settled.expressions.get(site).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Site(site), own, of))
            .unwrap_or_else(|| shape.typed_expression(site))
    }

    /// The cell of the type expression `expression` of `shape`.
    pub(super) fn expression_cell(
        &self,
        shape: &BodyShape<'graph>,
        expression: &TypeExpression<'graph>,
    ) -> StaticType<'graph> {
        match self.resume {
            Some(_) => self.type_cell(shape, expression.site),
            None => expression.typed(),
        }
    }

    /// The cell of the callable body `body`'s type.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'graph> {
        let own = |cell| match cell {
            Rerun::Callable(typed) => Some(typed),
            _ => None,
        };
        (self.settled_cell(body, FactKey::Shape, own, |settled| settled.callable))
            .unwrap_or_else(|| body.callable_type())
    }

    /// The callable body `body`'s own group as its body reads it.
    pub(super) fn group_levels(&self, body: &BodyShape<'graph>) -> &'graph [Parametric] {
        let Some(resume) = &self.resume else {
            return body.group_levels();
        };
        Resume::found(&resume.levels, self.scratch, body.segment())
            .or_else(|| (resume.over.channel(body.segment())).find_map(|settled| settled.levels))
            .unwrap_or_else(|| body.group_levels())
    }

    /// The callable body `body`'s function type as its body reads it, its own group at its group
    /// levels: `None` where the channel did not type it.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>) -> Option<Parametric> {
        let (callable, levels) = (self.callable_cell(body), self.group_levels(body));
        in_body_of(callable, levels, self.types, self.scratch)
    }

    /// Whether the quote's code `code` holds a refusal.
    pub(super) fn code_refused(&self, code: &BodyShape<'graph>) -> bool {
        code.refusal().is_some()
            || self.resume.as_ref().is_some_and(|resume| {
                Resume::found(&resume.refusals, self.scratch, code.segment()).is_some()
                    || (resume.over.channel(code.segment())).any(|held| held.refusal.is_some())
            })
    }

    /// Every type expression `shape` records, each cell the walk fixes.
    pub(super) fn type_expressions(
        &self,
        shape: &BodyShape<'graph>,
    ) -> &'graph [TypeExpression<'graph>] {
        shape.type_expressions()
    }

    /// Every cell of `shape` the walk fixed that waits on an unknown, once the walk has visited
    /// `shape`.
    pub(super) fn residual_of(&self, shape: &BodyShape<'graph>) -> Residual<'graph> {
        let Some(resume) = &self.resume else {
            return shape.channel_residual();
        };
        Resume::found(&resume.residuals, self.scratch, shape.segment())
            .or_else(|| (resume.over.channel(shape.segment()).next()).map(|held| held.residual))
            .unwrap_or_else(|| shape.channel_residual())
    }

    /// Lay `shape`'s residual down.
    pub(super) fn fix_channel_residual(
        &self,
        shape: &BodyShape<'graph>,
        residual: Residual<'graph>,
    ) {
        match &self.resume {
            Some(resume) => edit(&resume.residuals, self.scratch, |held| {
                held.push((shape.segment(), residual))
            }),
            None if residual.is_empty() => {}
            None => shape.fix_channel_residual(residual),
        }
    }

    /// Fix `expression`'s cell, one of `shape`'s.
    pub(super) fn fix_expression(
        &self,
        shape: &BodyShape<'graph>,
        expression: &TypeExpression<'graph>,
        typed: StaticType<'graph>,
    ) {
        match &self.resume {
            Some(resume) => resume.rerun(
                shape.segment(),
                FactKey::Site(expression.site),
                Rerun::Expression(typed),
            ),
            None => expression.fix(typed),
        }
    }

    /// Fix the cell of the type binder at `slot` of `shape`.
    pub(super) fn fix_declared(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
        typed: StaticType<'graph>,
    ) {
        match &self.resume {
            Some(resume) => {
                resume.rerun(shape.segment(), FactKey::Slot(slot), Rerun::Declared(typed))
            }
            None => shape.fix_declared(slot, typed),
        }
    }

    /// Fix the cell of the registration at `slot` of `shape`.
    pub(super) fn fix_registered(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'graph>,
    ) {
        match &self.resume {
            Some(resume) => resume.rerun(
                shape.segment(),
                FactKey::Slot(slot),
                Rerun::Registered(typed),
            ),
            None => shape.fix_registered(slot, typed),
        }
    }

    /// Fix the callable body `body`'s cell.
    pub(super) fn fix_callable(&self, body: &BodyShape<'graph>, typed: StaticCallable<'graph>) {
        match &self.resume {
            Some(resume) => resume.rerun(body.segment(), FactKey::Shape, Rerun::Callable(typed)),
            None => body.fix_callable(typed),
        }
    }

    /// Fix the callable body `body`'s own group as its body reads it.
    pub(super) fn fix_group_levels(&self, body: &BodyShape<'graph>, levels: &'graph [Parametric]) {
        match &self.resume {
            Some(resume) => edit(&resume.levels, self.scratch, |held| {
                held.push((body.segment(), levels))
            }),
            None => body.fix_group_levels(levels),
        }
    }

    /// Keep the refusal of the quote's code `code`.
    pub(super) fn refuse_code(
        &self,
        code: &BodyShape<'graph>,
        refusal: &'graph ShapeError<'graph>,
    ) {
        match &self.resume {
            Some(resume) => edit(&resume.refusals, self.scratch, |held| {
                held.push((code.segment(), refusal))
            }),
            None => code.refuse_typing(refusal),
        }
    }

    /// Lay the path spellings `shape`'s type expressions read down: the load's, which a
    /// resumption reads alike.
    pub(super) fn fix_path_reads(
        &self,
        shape: &BodyShape<'graph>,
        reads: Keyed<'graph, (Site, PathRead<'graph>)>,
    ) {
        if self.resume.is_none() {
            shape.fix_path_reads(reads);
        }
    }

    /// The path entries the channel minted for the chain `root` roots, at the load.
    pub(super) fn loaded_paths(
        &self,
        root: &BodyShape<'graph>,
    ) -> &'graph [&'graph PathEntry<'graph>] {
        root.channel_paths()
    }

    /// Lay the path entries the channel minted for the chain `root` roots down.
    pub(super) fn fix_channel_paths(
        &self,
        root: &BodyShape<'graph>,
        paths: &'graph [&'graph PathEntry<'graph>],
    ) {
        if self.resume.is_none() {
            root.fix_channel_paths(paths);
        }
    }

    /// Lay each lexical variable `shape` declares down, beside where its activation holds it.
    pub(super) fn fix_declared_variables(
        &self,
        shape: &BodyShape<'graph>,
        variables: &'graph [(usize, Target)],
    ) {
        if self.resume.is_none() {
            shape.fix_declared_variables(variables);
        }
    }

    /// Whether the cell the walk is about to type, waiting on `frontier`, is one a resumption
    /// skips: its frontier waits on nothing it binds. Counts the cell where it re-runs.
    pub(super) fn skips(&self, frontier: crate::scope::Frontier<'_>) -> bool {
        let Some(resume) = &self.resume else {
            return false;
        };
        if !resume.over.wakes(frontier) {
            return true;
        }
        resume.reran.set(resume.reran.get() + 1);
        false
    }

    /// Whether a resumption skips the shape `nested` whole: no fact of it, nor of a shape nested
    /// in it, waits on what it binds.
    pub(super) fn skips_shape(&self, nested: &BodyShape<'graph>) -> bool {
        self.resume
            .as_ref()
            .is_some_and(|resume| !resume.over.wakes(nested.reach()))
    }

    /// The type a resumption binds `unknown` to.
    pub(super) fn bound(
        &self,
        unknown: crate::scope::Unknown,
    ) -> Option<crate::type_lattice::KType> {
        self.resume.as_ref()?.over.bound(unknown)
    }
}

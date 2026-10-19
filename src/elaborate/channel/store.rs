//! The type channel's **fact store**: the one door every read and write of a channel cell goes
//! through ([the build module's design](../../build/README.md#type-facts-and-the-solver)). The
//! walk reaches a type binder's, a registration's, a type expression's or a callable's cell only
//! through the methods here, which `tests::boundary` holds it to, so the store is the one place a
//! read is recorded.
//!
//! The store has a read half and a write half. The read half is one set of methods, the load's and
//! a resumption's alike: it reads each cell as the walk has it — re-run by this walk, else as the
//! resumptions it extends settled it, else the load's. The write half is a [`Write`]: [`Load`]
//! relays each fact it is handed out of the walk's scratch into program storage and writes the
//! shape's cell; [`Resume`] keeps each in its own tables, beside the load's cells and never in
//! them, and relays what it keeps into its `Settled` once the walk ends. The rules lay into the
//! walk's scratch and never ask which half they run over.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, bump_set, bump_table};
use crate::scope::{
    BodyShape, ChannelSettled, FactKey, Frontier, Keyed, PathEntry, PathRead, Relay, Residual,
    Segment, Settled, ShapeError, Site, Slot, StaticCallable, StaticRegistered, StaticType, Target,
    TypeExpression, Unknown, in_body_of,
};
use crate::type_lattice::{Habitat, KType, Parametric};

use super::Pass;

/// A cell a resumption re-ran.
#[derive(Clone, Copy)]
pub(super) enum Rerun<'p> {
    Declared(StaticType<'p>),
    Registered(StaticRegistered<'p>),
    Expression(StaticType<'p>),
    Callable(StaticCallable<'p>),
}

/// The write half of the type channel's store: where each fact the walk fixes goes, and what the
/// walk itself wrote, which the read half reads first.
pub(super) trait Write<'p, 'graph> {
    fn fix_expression(
        &self,
        shape: &'graph BodyShape<'graph>,
        expression: &'graph TypeExpression<'graph>,
        typed: StaticType<'p>,
    );
    fn fix_declared(&self, shape: &'graph BodyShape<'graph>, slot: Slot, typed: StaticType<'p>);
    fn fix_registered(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'p>,
    );
    fn fix_callable(&self, body: &'graph BodyShape<'graph>, typed: StaticCallable<'p>);
    fn fix_group_levels(&self, body: &'graph BodyShape<'graph>, levels: &'p [Parametric]);
    fn refuse_code(&self, code: &'graph BodyShape<'graph>, refusal: ShapeError<'p>);
    fn fix_channel_residual(&self, shape: &'graph BodyShape<'graph>, residual: Residual<'p>);
    /// Each path spelling `shape` reads: the load's, which a resumption reads alike.
    fn fix_path_reads(&self, shape: &'graph BodyShape<'graph>, reads: Keyed<'p, (Site, PathRead)>);
    /// The path entries this walk minted for the chain `root` roots.
    fn fix_channel_paths(&self, root: &'graph BodyShape<'graph>, paths: &'p [PathEntry<'p>]);
    fn fix_declared_variables(
        &self,
        shape: &'graph BodyShape<'graph>,
        variables: &'p [(usize, Target)],
    );
    /// The cell at `key` of the shape at `segment` this walk wrote, where it wrote one beside the
    /// shape's own cells.
    fn own_cell(&self, segment: Segment, key: FactKey) -> Option<Rerun<'p>>;
    fn own_levels(&self, segment: Segment) -> Option<&'p [Parametric]>;
    fn own_residual(&self, segment: Segment) -> Option<Residual<'p>>;
    fn own_refusal(&self, segment: Segment) -> bool;
    /// Count a cell the walk re-runs.
    fn count(&self, segment: Segment, key: FactKey);
}

/// The load's write half: each fact relayed into program storage, into the shape's cell.
pub(super) struct Load<'relay, 'graph, 'w, 'from, H: Habitat<'from>> {
    pub(super) relay: &'relay Relay<'graph, 'w, 'from, H>,
}

impl<'p, 'graph, 'w, 'from, H: Habitat<'from>> Write<'p, 'graph>
    for Load<'_, 'graph, 'w, 'from, H>
{
    fn fix_expression(
        &self,
        _: &'graph BodyShape<'graph>,
        expression: &'graph TypeExpression<'graph>,
        typed: StaticType<'p>,
    ) {
        expression.fix(self.relay.static_type(typed));
    }

    fn fix_declared(&self, shape: &'graph BodyShape<'graph>, slot: Slot, typed: StaticType<'p>) {
        shape.fix_declared(slot, self.relay.static_type(typed));
    }

    fn fix_registered(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'p>,
    ) {
        shape.fix_registered(slot, self.relay.static_registered(typed));
    }

    fn fix_callable(&self, body: &'graph BodyShape<'graph>, typed: StaticCallable<'p>) {
        body.fix_callable(self.relay.static_callable(typed));
    }

    fn fix_group_levels(&self, body: &'graph BodyShape<'graph>, levels: &'p [Parametric]) {
        body.fix_group_levels(self.relay.levels(levels));
    }

    fn refuse_code(&self, code: &'graph BodyShape<'graph>, refusal: ShapeError<'p>) {
        code.refuse_typing(self.relay.shape_error_ref(&refusal));
    }

    fn fix_channel_residual(&self, shape: &'graph BodyShape<'graph>, residual: Residual<'p>) {
        if !residual.is_empty() {
            shape.fix_channel_residual(self.relay.residual(residual));
        }
    }

    fn fix_path_reads(&self, shape: &'graph BodyShape<'graph>, reads: Keyed<'p, (Site, PathRead)>) {
        shape.fix_path_reads(self.relay.path_reads(reads));
    }

    fn fix_channel_paths(&self, root: &'graph BodyShape<'graph>, paths: &'p [PathEntry<'p>]) {
        root.fix_channel_paths(self.relay.path_entries(paths));
    }

    fn fix_declared_variables(
        &self,
        shape: &'graph BodyShape<'graph>,
        variables: &'p [(usize, Target)],
    ) {
        shape.fix_declared_variables(self.relay.declared_variables(variables));
    }

    fn own_cell(&self, _: Segment, _: FactKey) -> Option<Rerun<'p>> {
        None
    }

    fn own_levels(&self, _: Segment) -> Option<&'p [Parametric]> {
        None
    }

    fn own_residual(&self, _: Segment) -> Option<Residual<'p>> {
        None
    }

    fn own_refusal(&self, _: Segment) -> bool {
        false
    }

    fn count(&self, _: Segment, _: FactKey) {}
}

/// One shape's fact by its segment, the newest the walk wrote.
type BySegment<'p, T> = Cell<BumpBackedMap<'p, Segment, T>>;

/// A resumption's write half: each cell it re-ran, by its shape's segment and where the cell lies,
/// kept in the walk's scratch until [`settled`](Self::settled) relays them.
pub(super) struct Resume<'p> {
    /// Whether the resumption re-runs every cell, not only those its binding wakes.
    everything: bool,
    cells: Cell<BumpBackedMap<'p, (Segment, FactKey), Rerun<'p>>>,
    levels: BySegment<'p, &'p [Parametric]>,
    residuals: BySegment<'p, Residual<'p>>,
    refusals: BySegment<'p, &'p ShapeError<'p>>,
    /// Each cell the walk re-ran, by its shape's segment and key.
    reran: Cell<BumpBackedSet<'p, (Segment, FactKey)>>,
    scratch: BumpAllocator<'p>,
}

impl<'p> Resume<'p> {
    pub(super) fn new(everything: bool, scratch: BumpAllocator<'p>) -> Self {
        Resume {
            everything,
            cells: Cell::new(bump_table(scratch)),
            levels: Cell::new(bump_table(scratch)),
            residuals: Cell::new(bump_table(scratch)),
            refusals: Cell::new(bump_table(scratch)),
            reran: Cell::new(bump_set(scratch)),
            scratch,
        }
    }

    fn rerun(&self, segment: Segment, key: FactKey, cell: Rerun<'p>) {
        let mut cells = self.cells.replace(bump_table(self.scratch));
        cells.insert((segment, key), cell);
        self.cells.set(cells);
    }

    /// What the walk wrote in `held` for `segment`.
    fn found<T: Copy>(&self, held: &BySegment<'p, T>, segment: Segment) -> Option<T> {
        let table = held.replace(bump_table(self.scratch));
        let found = table.get(&segment).copied();
        held.set(table);
        found
    }

    /// Write `value` for `segment` in `held`, in place of what the walk wrote there.
    fn write<T>(&self, held: &BySegment<'p, T>, segment: Segment, value: T) {
        let mut table = held.replace(bump_table(self.scratch));
        table.insert(segment, value);
        held.set(table);
    }

    /// What the walk settled under `binding`, already laid in the output — each shape's cells it
    /// re-ran, by segment — relayed out through `relay`.
    pub(super) fn settled<'out, 'w, 'from, H: Habitat<'from>>(
        &self,
        binding: crate::scope::Binding<'out>,
        relay: &Relay<'out, 'w, 'from, H>,
    ) -> Settled<'out> {
        let scratch = self.scratch;
        let (levels, residuals, refusals) = (
            self.levels.replace(bump_table(scratch)),
            self.residuals.replace(bump_table(scratch)),
            self.refusals.replace(bump_table(scratch)),
        );
        let mut cells = BumpVec::new_in(scratch);
        cells.extend(self.cells.replace(bump_table(scratch)));
        cells.sort_unstable_by_key(|(key, _)| *key);
        let mut segments = BumpVec::new_in(scratch);
        segments.extend(cells.iter().map(|((segment, _), _)| *segment));
        segments.extend(levels.keys().copied());
        segments.extend(residuals.keys().copied());
        segments.extend(refusals.keys().copied());
        segments.sort_unstable();
        segments.dedup();
        let mut settled = BumpVec::with_capacity_in(segments.len(), scratch);
        let mut at = 0;
        for segment in segments.iter().copied() {
            let (mut declared, mut registered, mut expressions) = (
                BumpVec::new_in(scratch),
                BumpVec::new_in(scratch),
                BumpVec::new_in(scratch),
            );
            let mut callable = None;
            // `cells` is sorted by segment, as `segments` is.
            while let Some(((held, key), cell)) = cells.get(at).copied()
                && held == segment
            {
                match (key, cell) {
                    (FactKey::Slot(slot), Rerun::Declared(typed)) => declared.push((slot, typed)),
                    (FactKey::Slot(slot), Rerun::Registered(typed)) => {
                        registered.push((slot, typed))
                    }
                    (FactKey::Site(site), Rerun::Expression(typed)) => {
                        expressions.push((site, typed))
                    }
                    (FactKey::Shape, Rerun::Callable(typed)) => callable = Some(typed),
                    _ => {}
                }
                at += 1;
            }
            settled.push((
                segment,
                ChannelSettled {
                    declared: Keyed::sorted(scratch, &mut declared),
                    registered: Keyed::sorted(scratch, &mut registered),
                    expressions: Keyed::sorted(scratch, &mut expressions),
                    callable,
                    levels: levels.get(&segment).copied(),
                    residual: (residuals.get(&segment).copied()).unwrap_or(Keyed::from_sorted(&[])),
                    refusal: refusals.get(&segment).copied(),
                },
            ));
        }
        debug_assert!(
            (binding.entries().iter()).all(|(_, bound)| relay.types().resolves(*bound)),
            "a settled binding's handles resolve through the output chain"
        );
        let mut laid = Settled::empty(crate::scope::Binding::EMPTY);
        laid.channel = Keyed::from_sorted(scratch.alloc_slice_copy(&settled));
        laid.reran.0 = self.reran.replace(bump_set(scratch)).len() as u32;
        Settled {
            binding,
            ..relay.settled(laid)
        }
    }
}

impl<'p, 'graph> Write<'p, 'graph> for Resume<'p> {
    fn fix_expression(
        &self,
        shape: &'graph BodyShape<'graph>,
        expression: &'graph TypeExpression<'graph>,
        typed: StaticType<'p>,
    ) {
        let key = FactKey::Site(expression.site);
        self.rerun(shape.segment(), key, Rerun::Expression(typed));
    }

    fn fix_declared(&self, shape: &'graph BodyShape<'graph>, slot: Slot, typed: StaticType<'p>) {
        self.rerun(shape.segment(), FactKey::Slot(slot), Rerun::Declared(typed));
    }

    fn fix_registered(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'p>,
    ) {
        self.rerun(
            shape.segment(),
            FactKey::Slot(slot),
            Rerun::Registered(typed),
        );
    }

    fn fix_callable(&self, body: &'graph BodyShape<'graph>, typed: StaticCallable<'p>) {
        self.rerun(body.segment(), FactKey::Shape, Rerun::Callable(typed));
    }

    fn fix_group_levels(&self, body: &'graph BodyShape<'graph>, levels: &'p [Parametric]) {
        self.write(&self.levels, body.segment(), levels);
    }

    fn refuse_code(&self, code: &'graph BodyShape<'graph>, refusal: ShapeError<'p>) {
        let refusal = crate::memory::resident(self.scratch, refusal);
        self.write(&self.refusals, code.segment(), refusal);
    }

    fn fix_channel_residual(&self, shape: &'graph BodyShape<'graph>, residual: Residual<'p>) {
        self.write(&self.residuals, shape.segment(), residual);
    }

    fn fix_path_reads(&self, _: &'graph BodyShape<'graph>, _: Keyed<'p, (Site, PathRead)>) {}

    fn fix_channel_paths(&self, _: &'graph BodyShape<'graph>, paths: &'p [PathEntry<'p>]) {
        assert!(paths.is_empty(), "a resumption mints no path entry");
    }

    fn fix_declared_variables(&self, _: &'graph BodyShape<'graph>, _: &'p [(usize, Target)]) {}

    fn own_cell(&self, segment: Segment, key: FactKey) -> Option<Rerun<'p>> {
        let cells = self.cells.replace(bump_table(self.scratch));
        let held = cells.get(&(segment, key)).copied();
        self.cells.set(cells);
        held
    }

    fn own_levels(&self, segment: Segment) -> Option<&'p [Parametric]> {
        self.found(&self.levels, segment)
    }

    fn own_residual(&self, segment: Segment) -> Option<Residual<'p>> {
        self.found(&self.residuals, segment)
    }

    fn own_refusal(&self, segment: Segment) -> bool {
        self.found(&self.refusals, segment).is_some()
    }

    /// A cell re-runs at most once in a resumption that skips.
    fn count(&self, segment: Segment, key: FactKey) {
        let mut reran = self.reran.replace(bump_set(self.scratch));
        let new = reran.insert((segment, key));
        self.reran.set(reran);
        debug_assert!(
            new || self.everything,
            "a cell re-runs at most once in a resumption"
        );
    }
}

impl<'p, 'graph, X> Pass<'p, 'graph, '_, '_, '_, '_, X> {
    /// The cell this walk wrote at `key` of `shape`, else the newest the resumptions it extends
    /// settled, read through `of`; `None` where neither did — at the load always.
    fn settled_cell<T>(
        &self,
        shape: &BodyShape<'graph>,
        key: FactKey,
        own: impl Fn(Rerun<'p>) -> Option<T>,
        of: impl Fn(&ChannelSettled<'p>) -> Option<T>,
    ) -> Option<T> {
        if let Some(found) = self.write.own_cell(shape.segment(), key).and_then(own) {
            return Some(found);
        }
        self.over?.channel(shape.segment()).find_map(of)
    }

    /// The cell of the type binder at `slot` of `shape`.
    pub(super) fn declared_cell(&self, shape: &BodyShape<'graph>, slot: Slot) -> StaticType<'p> {
        let own = |cell| match cell {
            Rerun::Declared(typed) => Some(typed),
            _ => None,
        };
        let of = |settled: &ChannelSettled<'p>| settled.declared.get(slot).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Slot(slot), own, of))
            .unwrap_or_else(|| shape.declared_type(slot))
    }

    /// The cell of the type binder at `slot` of `shape` as the load fixed it, which numbers the
    /// chain's lexical variables alike at every resumption.
    pub(super) fn loaded_declared(&self, shape: &BodyShape<'graph>, slot: Slot) -> StaticType<'p> {
        shape.declared_type(slot)
    }

    /// The cell of the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'p> {
        let own = |cell| match cell {
            Rerun::Registered(typed) => Some(typed),
            _ => None,
        };
        let of = |settled: &ChannelSettled<'p>| settled.registered.get(slot).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Slot(slot), own, of))
            .unwrap_or_else(|| shape.registered_type(slot))
    }

    /// The cell of the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'p> {
        let own = |cell| match cell {
            Rerun::Expression(typed) => Some(typed),
            _ => None,
        };
        let of = |settled: &ChannelSettled<'p>| settled.expressions.get(site).map(|(_, at)| *at);
        (self.settled_cell(shape, FactKey::Site(site), own, of))
            .unwrap_or_else(|| shape.typed_expression(site))
    }

    /// The cell of the callable body `body`'s type.
    pub(super) fn callable_cell(&self, body: &BodyShape<'graph>) -> StaticCallable<'p> {
        let own = |cell| match cell {
            Rerun::Callable(typed) => Some(typed),
            _ => None,
        };
        (self.settled_cell(body, FactKey::Shape, own, |settled| settled.callable))
            .unwrap_or_else(|| body.callable_type())
    }

    /// The callable body `body`'s own group as its body reads it.
    pub(super) fn group_levels(&self, body: &BodyShape<'graph>) -> &'p [Parametric] {
        let segment = body.segment();
        (self.write.own_levels(segment))
            .or_else(|| (self.over?.channel(segment)).find_map(|settled| settled.levels))
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
        let segment = code.segment();
        code.refusal().is_some()
            || self.write.own_refusal(segment)
            || (self.over)
                .is_some_and(|over| over.channel(segment).any(|held| held.refusal.is_some()))
    }

    /// Every type expression `shape` records, each cell the walk fixes.
    pub(super) fn type_expressions(
        &self,
        shape: &'graph BodyShape<'graph>,
    ) -> &'graph [TypeExpression<'graph>] {
        shape.type_expressions()
    }

    /// Every cell of `shape` the walk fixed that waits on an unknown, once the walk has visited
    /// `shape`.
    pub(super) fn residual_of(&self, shape: &BodyShape<'graph>) -> Residual<'p> {
        let segment = shape.segment();
        (self.write.own_residual(segment))
            .or_else(|| (self.over?.channel(segment).next()).map(|held| held.residual))
            .unwrap_or_else(|| shape.channel_residual())
    }

    /// The residual a walk of `shape` starts from: each cell a resumption skips keeps its
    /// frontier. Nothing at the load, which skips nothing.
    pub(super) fn starting_residual(&self, shape: &BodyShape<'graph>) -> Residual<'p> {
        match self.over {
            Some(_) => self.residual_of(shape),
            None => Keyed::from_sorted(&[]),
        }
    }

    /// The path entries the channel minted for the chain `root` roots, at the load: none yet
    /// while the load walks it.
    pub(super) fn loaded_paths(&self, root: &BodyShape<'graph>) -> &'p [PathEntry<'p>] {
        root.channel_paths()
    }

    pub(super) fn fix_expression(
        &self,
        shape: &'graph BodyShape<'graph>,
        expression: &'graph TypeExpression<'graph>,
        typed: StaticType<'p>,
    ) {
        self.write.fix_expression(shape, expression, typed);
    }

    pub(super) fn fix_declared(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        typed: StaticType<'p>,
    ) {
        self.write.fix_declared(shape, slot, typed);
    }

    pub(super) fn fix_registered(
        &self,
        shape: &'graph BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'p>,
    ) {
        self.write.fix_registered(shape, slot, typed);
    }

    pub(super) fn fix_callable(&self, body: &'graph BodyShape<'graph>, typed: StaticCallable<'p>) {
        self.write.fix_callable(body, typed);
    }

    pub(super) fn fix_group_levels(
        &self,
        body: &'graph BodyShape<'graph>,
        levels: &'p [Parametric],
    ) {
        self.write.fix_group_levels(body, levels);
    }

    pub(super) fn refuse_code(&self, code: &'graph BodyShape<'graph>, refusal: ShapeError<'p>) {
        self.write.refuse_code(code, refusal);
    }

    pub(super) fn fix_channel_residual(
        &self,
        shape: &'graph BodyShape<'graph>,
        residual: Residual<'p>,
    ) {
        self.write.fix_channel_residual(shape, residual);
    }

    pub(super) fn fix_path_reads(
        &self,
        shape: &'graph BodyShape<'graph>,
        reads: Keyed<'p, (Site, PathRead)>,
    ) {
        self.write.fix_path_reads(shape, reads);
    }

    pub(super) fn fix_channel_paths(
        &self,
        root: &'graph BodyShape<'graph>,
        paths: &'p [PathEntry<'p>],
    ) {
        self.write.fix_channel_paths(root, paths);
    }

    pub(super) fn fix_declared_variables(
        &self,
        shape: &'graph BodyShape<'graph>,
        variables: &'p [(usize, Target)],
    ) {
        self.write.fix_declared_variables(shape, variables);
    }

    /// Whether the cell at `key` of `shape` the walk is about to type, waiting on `frontier`, is
    /// one a resumption skips: its frontier waits on nothing it binds. Counts the cell where it
    /// re-runs — once, since a re-run cell waits on nothing it binds.
    pub(super) fn skips(
        &self,
        shape: &BodyShape<'graph>,
        key: FactKey,
        frontier: Frontier<'_>,
    ) -> bool {
        let Some(over) = self.over else {
            return false;
        };
        if !over.wakes(frontier) {
            return true;
        }
        self.write.count(shape.segment(), key);
        false
    }

    /// Whether a resumption skips the shape `nested` whole: no fact of it, nor of a shape nested
    /// in it, waits on what it binds.
    pub(super) fn skips_shape(&self, nested: &BodyShape<'graph>) -> bool {
        (self.over).is_some_and(|over| !over.wakes(nested.reach()))
    }

    /// The type a resumption binds `unknown` to.
    pub(super) fn bound(&self, unknown: Unknown) -> Option<KType> {
        self.over?.bound(unknown)
    }
}

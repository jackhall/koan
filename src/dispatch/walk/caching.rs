//! What the caching fold reads of each site the walk types: every minting site and every composite
//! site of a body that may cache, beside what its content depends on
//! ([where each body interns](../../../design/type-tiers.md#where-each-body-interns)).
//!
//! A site's dependencies are the frame body's parameters its content trees bind, and the path
//! entries its parts' static types name, which the fold resolves through the records'
//! resolutions. Each is recorded as a [`SiteDeps`] in the record of the unit typing the site. The
//! fold classes every site once the walk has run, over the shape's invariance and each body's
//! closing class (the build's fold): the walk records no tail edge and no escape, since both
//! are the text's.

use crate::memory::{BumpVec, collect};
use crate::parse::{ExpressionPart, ListItem};
use crate::scope::{
    Binder, Coordinate, Deps, DepsGathering, Segment, ShapeKind, Site, SiteDeps, Slot, Store, Tree,
    tree_deps,
};
use crate::type_lattice::{Interval, KType, Side, Variable, read_through};

use super::{Form, Pass, of_part, porous_site};

/// What a site reads as the walk types it: what it depends on already, beside the paths it reads,
/// each by its chain root and level, which the fold resolves.
#[derive(Clone, Copy)]
pub(super) struct Reads<'o> {
    deps: Deps<'o>,
    pending: &'o [(Segment, usize)],
}

impl<'o> Reads<'o> {
    const CLOSED: Self = Reads {
        deps: Deps::Closed,
        pending: &[],
    };

    fn known(deps: Deps<'o>) -> Self {
        Reads { deps, pending: &[] }
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// The chain level of the frame body the shape at `level` runs in: the nearest at or below it
    /// that is the program, a callable or a quote's code.
    pub(super) fn frame_level(&self, level: usize) -> usize {
        (0..=level)
            .rev()
            .find(|at| {
                matches!(
                    self.chain[*at].shape.kind(),
                    ShapeKind::Program | ShapeKind::Callable | ShapeKind::Code
                )
            })
            .expect("the chain's first shape is a frame body")
    }

    /// Whether a site in the shape at `level` is recorded: one whose frame may cache, typed once.
    fn records(&self, level: usize) -> bool {
        !self.unfilled
            && matches!(
                self.chain[self.frame_level(level)].shape.kind(),
                ShapeKind::Callable | ShapeKind::Code
            )
    }

    /// The shape at `level`'s segment and its frame body's.
    fn holding(&self, level: usize) -> (Segment, Segment) {
        (
            self.chain[level].shape.segment(),
            self.chain[self.frame_level(level)].shape.segment(),
        )
    }

    /// Record `site` in the shape at `level` in the unit running.
    fn record_site(&mut self, level: usize, site: Site, minting: bool, reads: Reads<'o>) {
        let (holder, frame) = self.holding(level);
        let tag = self.innermost();
        self.units[tag.0 as usize].sites.push(SiteDeps {
            holder,
            frame,
            site,
            minting,
            deps: reads.deps,
            pending: reads.pending,
        });
    }

    /// Record the `:|` site at `site` in the shape at `level`, over its operand's content tree.
    pub(super) fn record_view(&mut self, level: usize, site: Site, tree: &Tree<'_>) {
        if !self.records(level) {
            return;
        }
        let mut deps = DepsGathering::new(self.scratch);
        if !tree.closed() {
            tree_deps(
                self.shapes,
                self.holding(level),
                tree,
                &mut deps,
                self.scratch,
            );
        }
        let deps = deps.laid(self.lay);
        self.record_site(level, site, true, Reads::known(deps));
    }

    /// Record the nominal component declared at `site` in the shape at `level`, over the names its
    /// members' declarations read, each member's own name aside: a type a run binds is read
    /// through the binder it names.
    pub(super) fn record_nominal(&mut self, level: usize, site: Site, members: &[Slot]) {
        if !self.records(level) {
            return;
        }
        let (shape, scratch) = (self.chain[level].shape, self.scratch);
        let mut deps = DepsGathering::new(scratch);
        let mut parts: BumpVec<'_, &'graph ExpressionPart<'graph>> = BumpVec::new_in(scratch);
        for member in members {
            if let Some(declaration) = shape.declarations(*member) {
                parts.extend(declaration.parts.iter().map(|part| &part.value));
            }
        }
        while let Some(part) = parts.pop() {
            match part {
                ExpressionPart::Expression(node)
                | ExpressionPart::SigiledTypeExpr(node)
                | ExpressionPart::RecordType(node)
                | ExpressionPart::MarkedUse(_, node) => {
                    parts.extend(node.parts.iter().map(|part| &part.value));
                }
                ExpressionPart::ListLiteral(items) => {
                    parts.extend(items.iter().map(ListItem::part))
                }
                ExpressionPart::DictLiteral(pairs) => {
                    parts.extend(pairs.iter().flat_map(|(key, value)| [key, value]));
                }
                ExpressionPart::RecordLiteral(fields) => {
                    parts.extend(fields.iter().map(|(_, value)| value));
                }
                _ => {
                    let Some(mention) = shape.mention(shape.site(part)) else {
                        continue;
                    };
                    let landed = self.landed_slot(level, mention.coordinate);
                    if landed.is_some_and(|(at, slot)| at == level && members.contains(&slot)) {
                        continue;
                    }
                    self.type_binder(level, mention.coordinate, &mut deps);
                }
            }
        }
        let deps = deps.laid(self.lay);
        self.record_site(level, site, true, Reads::known(deps));
    }

    /// Gather what the type binder `coordinate` names, read in the shape at `level`, depends on.
    fn type_binder(&self, level: usize, coordinate: Coordinate, into: &mut DepsGathering<'_>) {
        let Some((at, slot)) = self.landed_slot(level, coordinate) else {
            return into.add(Deps::Varying);
        };
        let binder = Binder {
            body: self.chain[at].shape.segment(),
            slot,
        };
        let leaf = Tree::bound(binder, &[], None);
        tree_deps(self.shapes, self.holding(level), &leaf, into, self.scratch);
    }

    /// Record the composite site at `site` in the shape at `level`, built over `parts`, each beside
    /// its static type.
    pub(super) fn record_composite(
        &mut self,
        level: usize,
        site: Site,
        parts: impl Iterator<Item = (&'graph ExpressionPart<'graph>, Interval)>,
    ) {
        if !self.records(level) {
            return;
        }
        let scratch = self.scratch;
        let mut deps = DepsGathering::new(scratch);
        let mut pending: BumpVec<'_, &'o [(Segment, usize)]> = BumpVec::new_in(scratch);
        for (part, typed) in parts {
            let each = self.part_reads(level, part, typed);
            deps.add(each.deps);
            if !each.pending.is_empty() {
                pending.push(each.pending);
            }
        }
        // One part's pending entries are its slice; several parts' are laid together.
        let pending = match pending.as_slice() {
            [] => &[][..],
            [only] => *only,
            several => {
                let mut all = BumpVec::new_in(scratch);
                several.iter().for_each(|each| all.extend_from_slice(each));
                collect(self.lay, all.iter().copied())
            }
        };
        let reads = Reads {
            deps: deps.laid(self.lay),
            pending,
        };
        let segment = self.chain[level].shape.segment();
        self.composites.insert((segment, site), reads);
        self.record_site(level, site, false, reads);
    }

    /// What a composite's part reads: a composite site's own reads; nothing where no value of its
    /// static type can reach a minted type; the path entries of the variables it names, where
    /// reading each as `Number` leaves it so; and anything otherwise — `Any`, `Value`, a type
    /// name's variable, a carrier the load cannot name.
    fn part_reads(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
        typed: Interval,
    ) -> Reads<'o> {
        let shape = self.chain[level].shape;
        let site = match of_part(shape, part) {
            Form::Apply(node) => shape.site_of_node(node),
            _ => shape.site(part),
        };
        if let Some(reads) = self.composites.get(&(shape.segment(), site)) {
            return *reads;
        }
        let (types, scratch) = (self.types, self.scratch);
        if !porous_site(types, typed) {
            return Reads::CLOSED;
        }
        let mut levels = BumpVec::new_in(scratch);
        let mut number = |variable: Variable| {
            if let Variable::Lexical { level, .. } = variable {
                levels.push(level);
            }
            Some(Interval::point(KType::NUMBER.into()))
        };
        let mapped = Interval {
            lower: read_through(types, scratch, typed.lower, Side::Below, &mut number),
            upper: read_through(types, scratch, typed.upper, Side::Above, &mut number),
        };
        if levels.is_empty() || porous_site(types, mapped) {
            return Reads::known(Deps::Varying);
        }
        let mut pending = BumpVec::with_capacity_in(levels.len(), scratch);
        for variable in levels {
            let entry = (self.is_path(level, variable))
                .then(|| self.reachable(level, variable))
                .flatten();
            match entry {
                Some((entry, _)) => pending.push((self.chain_root_segment(level), entry.level)),
                None => return Reads::known(Deps::Varying),
            }
        }
        Reads {
            deps: Deps::Closed,
            pending: collect(self.lay, pending.iter().copied()),
        }
    }
}

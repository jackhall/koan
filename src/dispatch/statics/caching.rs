//! Where a caching body's types intern at run: each minting site classed closed, invariant or
//! varying by what its content reads, each composite site classed by its parts, and the
//! invariance fixpoint over the tail-call graph that fixes the classes
//! ([where each body interns](../../../design/type-tiers.md#where-each-body-interns)).
//!
//! The pass records, as it types each shape, what every site reads ([`Reads`]): the frame body's
//! parameters its content trees bind, and the path entries its parts' static types name, which the
//! pass resolves later in the shape. It records every tail edge a callable's or a quote's code's
//! last statement makes, and every callable whose name escapes as a value. A site's class depends
//! on edges from every body in the program and on every path's resolution, so the classes are
//! fixed once the whole pass has run ([`Pass::cache`]), into each shape's write-once [`Caching`]
//! cell, beside the frame parameters its invariant sites read, which key its specialization. A wrong class costs growth or warmth, never soundness: an edge or an escape the pass
//! misses only keeps a parameter invariant too long, and one it over-counts only keeps a site
//! cold.

use crate::memory::{BumpAllocator, BumpBackedSet, BumpVec, bump_set, collect, resident};
use crate::parse::ExpressionPart;
use crate::scope::{
    BodyClass, BodyShape, Caching, Candidate, Coordinate, Keyed, Leaf, Mint, NodeCapture,
    ParameterBinding, PathEntry, Position, ShapeKind, Site, Slot, Static, Tree,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{Interval, KType, Side, Variable, read_through};

use super::{Form, Landing, Pass, Wanted, address, of_node, of_part, porous_site, slots};

/// What a site's content depends on: nothing that varies per hop born under one park, the frame
/// body's parameters at these slots — none, for content read through captures alone — or
/// something that varies regardless.
#[derive(Clone, Copy)]
pub(super) enum Deps<'p> {
    Closed,
    On(&'p [Slot]),
    Varying,
}

/// What a site reads as the pass types it: what it depends on already, beside the path entries it
/// reads, each resolved to its content tree once the pass has written it.
#[derive(Clone, Copy)]
pub(super) struct Reads<'p, 'graph> {
    deps: Deps<'p>,
    pending: &'p [&'graph PathEntry<'graph>],
}

/// One classed site of a frame body that may cache.
#[derive(Clone, Copy)]
pub(super) struct SiteRecord<'p, 'graph> {
    /// The shape the site sits in: the frame body, or a block or module body it runs.
    holder: &'graph BodyShape<'graph>,
    /// The frame body the site's dependencies name parameters of.
    frame: &'graph BodyShape<'graph>,
    site: Site,
    /// A minting site, or a composite site.
    minting: bool,
    reads: Reads<'p, 'graph>,
    /// The chain the site was read on, up to its holder, and the frame body's place on it.
    shapes: &'p [&'graph BodyShape<'graph>],
    frame_at: usize,
}

/// A tail call: the caller's frame body, the callee's body, and per parameter of the callee the
/// caller's own parameter passed bare at it, where one is.
#[derive(Clone, Copy)]
pub(super) struct Edge<'p, 'graph> {
    caller: &'graph BodyShape<'graph>,
    callee: &'graph BodyShape<'graph>,
    arguments: &'p [(Slot, Option<Slot>)],
}

impl<'p> Deps<'p> {
    /// The dependencies of content built over both.
    fn union(self, other: Self, scratch: BumpAllocator<'p>) -> Self {
        match (self, other) {
            (Deps::Varying, _) | (_, Deps::Varying) => Deps::Varying,
            (Deps::Closed, deps) | (deps, Deps::Closed) => deps,
            (Deps::On(mine), Deps::On(theirs)) => {
                let mut slots = BumpVec::with_capacity_in(mine.len() + theirs.len(), scratch);
                slots.extend_from_slice(mine);
                slots.extend_from_slice(theirs);
                slots.sort_unstable();
                slots.dedup();
                Deps::On(scratch.alloc_slice_copy(&slots))
            }
        }
    }
}

impl<'p, 'graph> Reads<'p, 'graph> {
    const CLOSED: Self = Reads {
        deps: Deps::Closed,
        pending: &[],
    };

    fn known(deps: Deps<'p>) -> Self {
        Reads { deps, pending: &[] }
    }

    fn union(self, other: Self, scratch: BumpAllocator<'p>) -> Self {
        let pending = match (self.pending, other.pending) {
            ([], theirs) => theirs,
            (mine, []) => mine,
            (mine, theirs) => {
                let mut both = BumpVec::with_capacity_in(mine.len() + theirs.len(), scratch);
                both.extend_from_slice(mine);
                both.extend_from_slice(theirs);
                scratch.alloc_slice_copy(&both)
            }
        };
        Reads {
            deps: self.deps.union(other.deps, scratch),
            pending,
        }
    }
}

impl<'p, 'graph> SiteRecord<'p, 'graph> {
    /// What the site depends on, each path entry it reads resolved to its content tree.
    fn deps(&self, scratch: BumpAllocator<'p>) -> Deps<'p> {
        let mut deps = self.reads.deps;
        for entry in self.reads.pending {
            let each = match entry.resolution() {
                Some(resolution) => {
                    tree_deps(self.shapes, self.frame_at, &resolution.tree, scratch)
                }
                None => Deps::Varying,
            };
            deps = deps.union(each, scratch);
        }
        deps
    }
}

/// What the content `tree`, read on the chain `shapes` whose frame body is at `frame`, depends on:
/// each leaf bound to a parameter of the frame body, no parameter for a leaf captured from a body
/// enclosing it, and anything for a leaf no binder names, one bound per run of a body, or one bound
/// by a non-parameter of the frame or of a block or module body it runs.
fn tree_deps<'p, 'graph>(
    shapes: &[&'graph BodyShape<'graph>],
    frame: usize,
    tree: &Tree<'graph>,
    scratch: BumpAllocator<'p>,
) -> Deps<'p> {
    let mut deps = Deps::Closed;
    let mut stack = BumpVec::new_in(scratch);
    stack.push(*tree);
    while let Some(tree) = stack.pop() {
        match tree {
            Tree::Digest(_) => {}
            Tree::Body { captures, .. } => stack.extend(captures.iter().map(|(_, at)| *at)),
            Tree::Knot { members, .. } => {
                for member in members {
                    stack.extend(
                        member
                            .captures
                            .iter()
                            .filter_map(|(_, capture)| match capture {
                                NodeCapture::Tree(tree) => Some(*tree),
                                NodeCapture::Fellow(_) => None,
                            }),
                    );
                }
            }
            Tree::View { source, .. } | Tree::Retyped { source, .. } => stack.push(*source),
            Tree::Leaf(Leaf::Unnamed(_)) => return Deps::Varying,
            Tree::Leaf(Leaf::Bound {
                within: Some(_), ..
            }) => return Deps::Varying,
            Tree::Leaf(Leaf::Bound { binder, .. }) => {
                let at = (0..shapes.len())
                    .rev()
                    .find(|at| std::ptr::eq(shapes[*at], binder.shape));
                let each = match at {
                    Some(at) => binder_deps(shapes, frame, at, binder.slot, scratch),
                    None => Deps::Varying,
                };
                deps = deps.union(each, scratch);
            }
        }
    }
    deps
}

/// What a binder at `slot` of the shape at `at` on the chain `shapes`, whose frame body is at
/// `frame`, depends on: a parameter of the frame body is itself; a capture from a body enclosing
/// the frame is no parameter — open, but the same at every frame of one closure; anything else
/// varies.
fn binder_deps<'p>(
    shapes: &[&BodyShape<'_>],
    frame: usize,
    at: usize,
    slot: Slot,
    scratch: BumpAllocator<'p>,
) -> Deps<'p> {
    if at < frame {
        return Deps::On(&[]);
    }
    if at == frame && shapes[at].position_of(slot) == Position::PARAMETER {
        return Deps::On(scratch.alloc_slice_copy(&[slot]));
    }
    Deps::Varying
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
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

    /// The chain's shapes up to `level`, which a site's reads are resolved against.
    fn shapes(&self, level: usize) -> &'p [&'graph BodyShape<'graph>] {
        (self.scratch).alloc_slice_fill_iter(self.chain[..=level].iter().map(|at| at.shape))
    }

    /// Whether a site in the shape at `level` is recorded: one whose frame may cache, typed once.
    fn records(&self, level: usize) -> bool {
        !self.unfilled
            && matches!(
                self.chain[self.frame_level(level)].shape.kind(),
                ShapeKind::Callable | ShapeKind::Code
            )
    }

    /// Note that the argument `part` of a call in the shape at `level` passes on a value its frame
    /// body received, where that body may cache.
    pub(super) fn note_received(&mut self, level: usize, part: &'graph ExpressionPart<'graph>) {
        if self.records(level) {
            self.received
                .push((self.chain[level].shape, Site::of(part)));
        }
    }

    /// Record `site` in the shape at `level`, whose chain up to it is `shapes`.
    fn record(
        &mut self,
        level: usize,
        site: Site,
        minting: bool,
        reads: Reads<'p, 'graph>,
        shapes: &'p [&'graph BodyShape<'graph>],
    ) {
        let frame_at = self.frame_level(level);
        self.sites.push(SiteRecord {
            holder: self.chain[level].shape,
            frame: self.chain[frame_at].shape,
            site,
            minting,
            reads,
            shapes,
            frame_at,
        });
    }

    /// Record the `:|` site at `site` in the shape at `level`, over its operand's content tree.
    pub(super) fn record_view(&mut self, level: usize, site: Site, tree: &Tree<'graph>) {
        if !self.records(level) {
            return;
        }
        let shapes = self.shapes(level);
        let deps = if tree.closed() {
            Deps::Closed
        } else {
            tree_deps(shapes, self.frame_level(level), tree, self.scratch)
        };
        self.record(level, site, true, Reads::known(deps), shapes);
    }

    /// Record the nominal component declared at `site` in the shape at `level`, over the names its
    /// members' declarations read, each member's own name aside: a type a run binds is read
    /// through the binder it names.
    pub(super) fn record_nominal(&mut self, level: usize, site: Site, members: &[Slot]) {
        if !self.records(level) {
            return;
        }
        let (shape, scratch) = (self.chain[level].shape, self.scratch);
        let mut deps = Deps::Closed;
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
                ExpressionPart::ListLiteral(items) => parts.extend(items.iter()),
                ExpressionPart::DictLiteral(pairs) => {
                    parts.extend(pairs.iter().flat_map(|(key, value)| [key, value]));
                }
                ExpressionPart::RecordLiteral(fields) => {
                    parts.extend(fields.iter().map(|(_, value)| value));
                }
                _ => {
                    let Some(mention) = shape.mention(Site::of(part)) else {
                        continue;
                    };
                    let landed = self.landed_slot(level, mention.coordinate);
                    if landed.is_some_and(|(at, slot)| at == level && members.contains(&slot)) {
                        continue;
                    }
                    deps = deps.union(self.type_binder(level, mention.coordinate), scratch);
                }
            }
        }
        self.record(level, site, true, Reads::known(deps), self.shapes(level));
    }

    /// What the type binder `coordinate` names, read in the shape at `level`, depends on.
    fn type_binder(&self, level: usize, coordinate: Coordinate) -> Deps<'p> {
        match self.landed_slot(level, coordinate) {
            Some((at, slot)) => binder_deps(
                self.shapes(level),
                self.frame_level(level),
                at,
                slot,
                self.scratch,
            ),
            None => Deps::Varying,
        }
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
        let mut reads = Reads::CLOSED;
        for (part, typed) in parts {
            let each = self.part_reads(level, part, typed);
            reads = reads.union(each, self.scratch);
        }
        self.composites.insert(site, reads);
        self.record(level, site, false, reads, self.shapes(level));
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
    ) -> Reads<'p, 'graph> {
        let site = match of_part(self.chain[level].shape, part) {
            Form::Apply(node) => Site::of_node(node),
            _ => Site::of(part),
        };
        if let Some(reads) = self.composites.get(&site) {
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
                Some((entry, _)) => pending.push(entry),
                None => return Reads::known(Deps::Varying),
            }
        }
        Reads {
            deps: Deps::Closed,
            pending: scratch.alloc_slice_copy(&pending),
        }
    }

    /// Mark the callable body `body` as escaping: its name is read as a value, or it is born
    /// somewhere a call by name cannot reach it from, so a frame of it may be born from anywhere.
    pub(super) fn escapes(&mut self, body: &'graph BodyShape<'graph>) {
        if !self.unfilled {
            self.escaping.insert(address(body));
        }
    }

    /// Record that the callable body `body` is born at a binder of the shape at `level`, whose
    /// kind decides whether a call by name reaches every frame of it.
    pub(super) fn born(&mut self, level: usize, body: &'graph BodyShape<'graph>) {
        if self.unfilled || body.kind() != ShapeKind::Callable {
            return;
        }
        match self.chain[level].shape.kind() {
            ShapeKind::Program | ShapeKind::Callable | ShapeKind::Code | ShapeKind::Block => {
                self.bound_at_binder.insert(address(body));
            }
            ShapeKind::Module => self.escapes(body),
        }
    }

    /// The callable body a name read in the shape at `level` through `coordinate` births, where
    /// it lands on a binder that births one.
    pub(super) fn births_callable(
        &self,
        level: usize,
        coordinate: Coordinate,
    ) -> Option<&'graph BodyShape<'graph>> {
        let Landing::Slot { level: at, slot } = self.follow(level, coordinate).landing else {
            return None;
        };
        (self.chain[at].shape.births(slot)).filter(|body| body.kind() == ShapeKind::Callable)
    }

    /// Record the tail edges of the last statement, `index`, of the frame body at `level`: a call
    /// by name of a callable a binder births, and one edge per candidate a keyworded use there may
    /// select, each beside the caller's own parameters it passes bare — a call by name's by its
    /// record literal's fields, a keyworded use's by slot position, as its registration names
    /// them.
    pub(super) fn tail_edges(&mut self, level: usize, index: usize) {
        let shape = self.chain[level].shape;
        if self.unfilled
            || !matches!(shape.kind(), ShapeKind::Callable | ShapeKind::Code)
            || index + 1 != shape.body().len()
        {
            return;
        }
        let scratch = self.scratch;
        let edge = |pass: &mut Self,
                    callee: &'graph BodyShape<'graph>,
                    passed: &dyn Fn(Slot) -> Option<Slot>| {
            let mut arguments = BumpVec::new_in(scratch);
            for slot in (0..callee.slots() as u32).map(Slot) {
                if callee.position_of(slot) == Position::PARAMETER {
                    arguments.push((slot, passed(slot)));
                }
            }
            pass.edges.push(Edge {
                caller: shape,
                callee,
                arguments: scratch.alloc_slice_copy(&arguments),
            });
        };
        match of_node(shape, &shape.body()[index]) {
            Form::Apply(node) => {
                let [head, argument] = node.parts else {
                    return;
                };
                let Form::Name(head) = of_part(shape, &head.value) else {
                    return;
                };
                let Some(callee) = (shape.mention(Site::of(head)))
                    .and_then(|mention| self.births_callable(level, mention.coordinate))
                else {
                    return;
                };
                let mut bare = BumpVec::new_in(scratch);
                if let ExpressionPart::RecordLiteral(fields) = &argument.value {
                    for (field, value) in fields.iter() {
                        if let Some(from) = self.bare_parameter(level, value) {
                            bare.push((field.symbol(), from));
                        }
                    }
                }
                let passed = |slot: Slot| {
                    let name = callee.slot_name(slot).symbol();
                    bare.iter()
                        .find(|(field, _)| *field == name)
                        .map(|(_, from)| *from)
                };
                edge(self, callee, &passed);
            }
            Form::Call(node, list) => {
                let wanted = slots(node, scratch);
                for candidate in list.candidates {
                    let Candidate::One(coordinate) = candidate else {
                        continue;
                    };
                    let Some(callee) = self.births_callable(level, *coordinate) else {
                        continue;
                    };
                    let mut bare = BumpVec::new_in(scratch);
                    let names = self.registered_names(level, *coordinate).unwrap_or(&[]);
                    for (name, wanted) in names.iter().zip(wanted.iter()) {
                        if let Wanted::Evaluated(part) = wanted
                            && let Some(from) = self.bare_parameter(level, part)
                        {
                            bare.push((*name, from));
                        }
                    }
                    let passed = |slot: Slot| {
                        let name = callee.slot_name(slot);
                        (bare.iter().find(|(each, _)| *each == name)).map(|(_, from)| *from)
                    };
                    edge(self, callee, &passed);
                }
            }
            _ => {}
        }
    }

    /// The parameter each slot of the registration `coordinate` lands on binds, in slot order:
    /// `None` for one the load has not typed, or that packs its slots into one list.
    fn registered_names(
        &self,
        level: usize,
        coordinate: Coordinate,
    ) -> Option<&'graph [BinderSymbol]> {
        let Landing::Slot { level: home, slot } = self.follow(level, coordinate).landing else {
            return None;
        };
        let parameters = match self.chain[home].shape.registered_type(slot) {
            Static::Closed(registered) => registered.parameters,
            Static::Rigid { value, .. } => value.parameters,
            Static::Unknown => return None,
        };
        match parameters {
            ParameterBinding::Named(names) => Some(names),
            ParameterBinding::Operands => None,
        }
    }

    /// The caller's own parameter `value` names bare, read in the frame body at `level`.
    fn bare_parameter(&self, level: usize, value: &'graph ExpressionPart<'graph>) -> Option<Slot> {
        let shape = self.chain[level].shape;
        let Form::Name(name) = of_part(shape, value) else {
            return None;
        };
        let mention = shape.mention(Site::of(name))?;
        match self.follow(level, mention.coordinate).landing {
            Landing::Slot { level: at, slot }
                if at == level && shape.position_of(slot) == Position::PARAMETER =>
            {
                Some(slot)
            }
            _ => None,
        }
    }

    /// Record that the shape at `level`, now fixed, runs in its frame body: whose class says
    /// whether it caches.
    pub(super) fn holds(&mut self, level: usize) {
        if !self.unfilled {
            let frame = self.chain[self.frame_level(level)].shape;
            self.holders.push((self.chain[level].shape, frame));
        }
    }

    /// Fix every typed shape's caching record: run the invariance fixpoint over the tail edges,
    /// class each recorded site by it, and lay each shape's record down.
    ///
    /// Every callable parameter starts invariant where its body is born at a binder a call by name
    /// reaches and never escapes, and falls varying where a tail edge into it passes anything but a
    /// bare invariant parameter of the caller there.
    pub(super) fn cache(&mut self) {
        let scratch = self.scratch;
        let mut varying: BumpBackedSet<'p, (usize, Slot)> = bump_set(scratch);
        let invariant_body = |pass: &Self, body: &BodyShape<'_>| {
            let at = address(body);
            pass.bound_at_binder.contains(&at) && !pass.escaping.contains(&at)
        };
        loop {
            let mut changed = false;
            for edge in self.edges.iter() {
                for (slot, passed) in edge.arguments {
                    let key = (address(edge.callee), *slot);
                    if varying.contains(&key) {
                        continue;
                    }
                    let falls = match passed {
                        None => true,
                        Some(from) => {
                            !invariant_body(self, edge.caller)
                                || varying.contains(&(address(edge.caller), *from))
                        }
                    };
                    if falls {
                        varying.insert(key);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // A site's class, beside the frame parameters an invariant one reads.
        let resolve = |pass: &Self, record: &SiteRecord<'p, 'graph>| match record.deps(scratch) {
            Deps::Closed => (Mint::Closed, &[][..]),
            Deps::Varying => (Mint::Varying, &[][..]),
            Deps::On(slots) => {
                let frame = record.frame;
                let held = slots.iter().all(|slot| {
                    invariant_body(pass, frame) && !varying.contains(&(address(frame), *slot))
                });
                if held {
                    (Mint::Invariant, slots)
                } else {
                    (Mint::Varying, &[][..])
                }
            }
        };
        // Per frame body, the parameters its invariant sites read, across every holder it runs:
        // what keys its specialization. A frame with an invariant site and no entry here reads
        // captures alone.
        let mut keyed_frames: BumpBackedSet<'p, usize> = bump_set(scratch);
        let mut keyed_slots = BumpVec::new_in(scratch);
        for record in self.sites.iter() {
            let (class, slots) = resolve(self, record);
            if class == Mint::Invariant {
                keyed_frames.insert(address(record.frame));
                keyed_slots.extend(slots.iter().map(|slot| (address(record.frame), *slot)));
            }
        }
        keyed_slots.sort_unstable();
        keyed_slots.dedup();
        self.sites.sort_by_key(|record| address(record.holder));
        self.received.sort_by_key(|(holder, _)| address(holder));
        for (holder, frame) in self.holders.iter() {
            let caches = matches!(frame.class(), Some(BodyClass::Sealed { caches: true, .. }));
            let (mut mints, mut warm) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
            let at = address(holder);
            let from = (self.sites).partition_point(|record| address(record.holder) < at);
            let to = (self.sites).partition_point(|record| address(record.holder) <= at);
            let from_received = (self.received).partition_point(|(each, _)| address(each) < at);
            let to_received = (self.received).partition_point(|(each, _)| address(each) <= at);
            let mut received = BumpVec::new_in(scratch);
            received.extend(
                self.received[from_received..to_received]
                    .iter()
                    .map(|(_, site)| *site),
            );
            received.sort_unstable();
            received.dedup();
            for record in self.sites[from..to].iter() {
                let (class, _) = resolve(self, record);
                if record.minting {
                    mints.push((record.site, class));
                } else if class == Mint::Invariant {
                    warm.push(record.site);
                }
            }
            let invariant = (caches && std::ptr::eq(*holder, *frame) && keyed_frames.contains(&at))
                .then(|| {
                    let from = keyed_slots.partition_point(|(each, _)| *each < at);
                    let to = keyed_slots.partition_point(|(each, _)| *each <= at);
                    collect(
                        self.writer,
                        keyed_slots[from..to].iter().map(|(_, slot)| *slot),
                    )
                });
            holder.fix_caching(resident(
                self.writer,
                Caching {
                    caches,
                    mints: Keyed::sorted(self.writer, &mut mints),
                    warm: Keyed::sorted(self.writer, &mut warm),
                    received: Keyed::sorted(self.writer, &mut received),
                    invariant,
                },
            ));
        }
    }
}

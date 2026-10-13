//! The static pass's paths ([design/modules.md § Comparing two paths](../../../design/modules.md#comparing-two-paths)):
//! each path entry of a chain resolved to the type it is at load, every cell the type channel fixed
//! read through those resolutions, and a module's static type pinned through its paths.
//!
//! A path **resolves** off the content tree of the module its root names, walked through its member
//! chain. It is **closed** — one type at every run — where the load knows that module's signature
//! exactly, from the root's exact static type or a closed opaque view's tree, and reads the path
//! off it as the run does ([`path_binding`]), where its application pins its label, or where it
//! reads a type member a body binds closed.
//! Any other path is its **class**: the path variable of the first path resolved over a tree like
//! its own, at the same label, through the same application. A path written in a type through a
//! module built in place that does not resolve closed refuses the load, since the run builds no
//! module there to read it from.
//!
//! The pass resolves the paths a shape's type expressions spell just before it types the unit
//! spelling them, in program order, an inline root typed there; a cell over a path variable not yet
//! resolved reads as unknown. A module's static type whose upper end leaves a head parameter
//! unpinned is pinned to the path through it, a path read through that application, so two member
//! reads through one module share a type.
//!
//! A path variable reaches a static type only where a path of its class is **reachable**: its root
//! a binder on the chain the reading shape reads through its blocks and builder captures. The pass
//! adds no capture for a path, so a static type read across a callable or module boundary reads
//! each variable no path of whose class is reachable through its bounds, and every path variable a
//! use records is located at a reachable root.

use crate::elaborate::{
    SignatureMember, applied_member, path_base, path_binding, signature_member,
};
use crate::knot::module::view::opaque_view_types;
use crate::memory::{BumpBackedMap, BumpVec, bump_hash_table, bump_table, collect};
use crate::parse::ExpressionPart;
use crate::scope::{
    Binder, BodyShape, Callable, CaptureSlot, CaptureSource, Coordinate, Leaf, PathEntry,
    PathIndex, Position, Print, Registered, Resolution, Resolved, Root, ShapeError, ShapeKind,
    Site, Source, Static, StaticCallable, StaticRegistered, StaticType, Target, Tree,
    Variable as Located, each_part, part_of,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Scheme, Side, TypeNode, Variable, read_through,
    substitute_levels,
};
use crate::values::digest::Operator;

use super::trees::Scope;
use super::{Bound, Pass, under, unknown};

type Read<'graph, T> = Result<T, ShapeError<'graph>>;

/// An application's pins, by parameter.
type Pins<'p> = BumpVec<'p, (BinderSymbol, Parametric)>;

/// One chain's path entries: the type channel's, then the pass's own, each beside its resolution
/// once the pass resolved it.
pub(super) struct ChainPaths<'p, 'graph> {
    root: &'graph BodyShape<'graph>,
    /// The first path level: past every name level of the chain.
    base: usize,
    /// How many entries the type channel minted.
    channel: usize,
    /// Every entry, by its level less `base`.
    entries: BumpVec<'p, (&'graph PathEntry<'graph>, Option<Resolution<'graph>>)>,
    /// Each entry's index in `entries`, by root, chain and label.
    found: PathIndex<'p>,
    /// Each class's representative's level, by the label, application and tree print its paths
    /// share.
    classes: BumpBackedMap<'p, (TypeSymbol, Option<Parametric>, Print), usize>,
    /// Each path resolved to a class, by the class's representative's level, then its own.
    members: BumpVec<'p, (usize, usize)>,
    /// Each entry being resolved, by level.
    resolving: BumpVec<'p, usize>,
}

impl<'graph> ChainPaths<'_, 'graph> {
    /// The entry at `level`, beside its resolution: `None` where `level` is a name level.
    fn at(&self, level: usize) -> Option<(&'graph PathEntry<'graph>, Option<Resolution<'graph>>)> {
        self.entries.get(level.checked_sub(self.base)?).copied()
    }
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
    /// Open the path table of the chain `root` roots, holding the type channel's entries.
    pub(super) fn open_paths(&mut self, root: &'graph BodyShape<'graph>) {
        let channel = root.paths();
        let mut entries = BumpVec::with_capacity_in(channel.len(), self.scratch);
        entries.extend(channel.iter().map(|entry| (*entry, None)));
        let mut found = bump_hash_table(self.scratch);
        for (index, entry) in channel.iter().enumerate() {
            let hash = PathEntry::hash_of(entry.root, entry.chain, entry.label);
            found.insert_unique(hash, index, |at: &usize| {
                let held = channel[*at];
                PathEntry::hash_of(held.root, held.chain, held.label)
            });
        }
        self.paths.push(ChainPaths {
            root,
            base: channel
                .first()
                .map_or_else(|| path_base(root), |entry| entry.level),
            channel: channel.len(),
            entries,
            found,
            classes: bump_table(self.scratch),
            members: BumpVec::new_in(self.scratch),
            resolving: BumpVec::new_in(self.scratch),
        });
    }

    /// Close the path table of the chain `root` roots, laying every entry and its resolution down
    /// unless the pass is typing unfilled code.
    pub(super) fn close_paths(&mut self, root: &'graph BodyShape<'graph>) {
        let table = self.paths.pop().expect("a chain's paths are open");
        debug_assert!(std::ptr::eq(table.root, root));
        if self.unfilled {
            return;
        }
        for (entry, resolution) in table.entries.iter() {
            if let Some(resolution) = resolution {
                entry.fix_resolution(*resolution);
            }
        }
        if table.entries.len() > table.channel {
            root.fix_paths(collect(
                self.writer,
                table.entries.iter().map(|(entry, _)| *entry),
            ));
        }
    }

    /// The chain level of the root of the chain `level` sits on.
    fn chain_root(&self, level: usize) -> usize {
        (0..=level)
            .rev()
            .find(|at| self.chain[*at].roots_chain)
            .expect("every chain has a root")
    }

    /// The index in `paths` of the table of the chain `level` sits on.
    fn table(&self, level: usize) -> usize {
        let root = self.chain[self.chain_root(level)].shape;
        (self.paths.iter())
            .rposition(|table| std::ptr::eq(table.root, root))
            .expect("a chain's paths are open while it is typed")
    }

    /// The path entry of the chain `level` sits on for `root`, `chain` and `label`, read through
    /// `through`: the one held already, or a new one at the chain's next path level.
    fn path(
        &mut self,
        level: usize,
        root: Root<'graph>,
        chain: &[ValueSymbol],
        label: TypeSymbol,
        through: Option<Parametric>,
    ) -> &'graph PathEntry<'graph> {
        let writer = self.writer;
        let table = self.table(level);
        let table = &mut self.paths[table];
        let hash = PathEntry::hash_of(root, chain, label);
        let entries = &table.entries;
        if let Some(at) = table.found.find(hash, |at| {
            let (entry, _) = entries[*at];
            entry.is(root, chain, label) && entry.through == through
        }) {
            return entries[*at].0;
        }
        let level = table.base + table.entries.len();
        let entry = PathEntry::new(writer, root, chain, label, level, through);
        table.found.insert_unique(hash, table.entries.len(), |at| {
            let (held, _) = entries[*at];
            PathEntry::hash_of(held.root, held.chain, held.label)
        });
        table.entries.push((entry, None));
        entry
    }

    /// Each path the shape at `level` spells in one of its statements, beside the statement's
    /// index: by statement, then as written.
    pub(super) fn spelled(&self, level: usize) -> BumpVec<'p, (usize, &'graph PathEntry<'graph>)> {
        let shape = self.chain[level].shape;
        let mut spelled = BumpVec::new_in(self.scratch);
        if shape.path_reads().is_empty() {
            return spelled;
        }
        for (index, node) in shape.body().iter().enumerate() {
            each_part(node, |part| {
                if let Some(read) = shape.path_read(Site::of(part)) {
                    spelled.push((index, read.entry));
                }
            });
        }
        spelled
    }

    /// Resolve each path the shape at `level` spells: those not resolved with their statement, in
    /// the order the type channel minted them.
    pub(super) fn resolve_spelled(&mut self, level: usize) -> Read<'graph, ()> {
        let shape = self.chain[level].shape;
        let mut entries: BumpVec<'_, &PathEntry<'graph>> =
            BumpVec::with_capacity_in(shape.path_reads().len(), self.scratch);
        entries.extend(shape.path_reads().iter().map(|(_, read)| read.entry));
        entries.sort_unstable_by_key(|entry| entry.level);
        for entry in entries {
            self.resolve_path(level, entry)?;
        }
        Ok(())
    }

    /// What `entry`, a path of the chain the shape at `level` sits on, resolves to, resolving it
    /// now where the pass has not: a path's root written in place is typed in that shape.
    pub(super) fn resolve_path(
        &mut self,
        level: usize,
        entry: &'graph PathEntry<'graph>,
    ) -> Read<'graph, Resolved> {
        let table = self.table(level);
        if let Some((_, Some(resolution))) = self.paths[table].at(entry.level) {
            return Ok(resolution.resolved);
        }
        // A path met while it is being resolved stands for itself.
        if self.paths[table].resolving.contains(&entry.level) {
            return Ok(Resolved::Class(entry.level));
        }
        self.paths[table].resolving.push(entry.level);
        let resolution = self.resolution(level, entry);
        self.paths[table].resolving.pop();
        let (resolution, at) = resolution?;
        let index = entry.level - self.paths[table].base;
        let channel = index < self.paths[table].channel;
        if let (Root::Inline(_), Resolved::Class(_), true, Some(at)) =
            (entry.root, resolution.resolved, channel, at)
        {
            return Err(ShapeError::OpenPathInType { at });
        }
        let table = &mut self.paths[table];
        table.entries[index].1 = Some(resolution);
        if let Resolved::Class(rep) = resolution.resolved {
            let at = table
                .members
                .partition_point(|member| *member < (rep, entry.level));
            table.members.insert(at, (rep, entry.level));
        }
        Ok(resolution.resolved)
    }

    /// `entry`'s resolution, read from `level`, beside where its root is written where it is
    /// written in place.
    fn resolution(
        &mut self,
        level: usize,
        entry: &'graph PathEntry<'graph>,
    ) -> Read<'graph, (Resolution<'graph>, Option<crate::source::SourceRef>)> {
        let (types, scratch) = (self.types, self.scratch);
        let (root, held, site, at) = match entry.root {
            Root::Binder(binder) => match self.binder_root(level, binder)? {
                Some((tree, held, site)) => (tree, held, site, None),
                // A binder off the chain is a leaf the load reads nothing through.
                None => {
                    let leaf = Tree::bound(binder, entry.chain, None);
                    let resolved = Resolved::Class(self.class_of(level, entry, leaf));
                    return Ok((
                        Resolution {
                            tree: leaf,
                            resolved,
                        },
                        None,
                    ));
                }
            },
            Root::Inline(site) => {
                let shape = self.chain[level].shape;
                let part = shape.body().iter().find_map(|node| part_of(node, site));
                match part {
                    Some(part) => {
                        let typed = match self.chain[level].parts.iter().find(|(at, _)| *at == site)
                        {
                            Some((_, typed)) => *typed,
                            None => self.part(level, part)?,
                        };
                        let tree = self.tree(level, part)?;
                        (
                            tree,
                            exact(types, typed),
                            site,
                            Some(self.source(level, site)),
                        )
                    }
                    None => (Tree::Leaf(Leaf::Unnamed(site)), None, site, None),
                }
            }
        };
        let mut tree = root;
        for step in entry.chain {
            tree = self
                .read_member(tree, *step, site)?
                .unwrap_or(Tree::Leaf(Leaf::Unnamed(site)));
        }
        let through = match entry.through {
            None => Some(None),
            Some(through) => types.concrete(through).map(Some),
        };
        // The signature of the module the root names, or the chain reaches, where the load knows
        // it exactly: the path is then what the run reads off it.
        let closed = through.and_then(|through| {
            let read = |held: KType, chain: &[ValueSymbol]| {
                path_binding(types, scratch, held, chain, entry.label, through)
            };
            held.and_then(|held| read(held, entry.chain))
                .or_else(|| match tree {
                    Tree::View {
                        operator: Operator::Opaque,
                        application,
                        source,
                        ..
                    } => source
                        .digest()
                        .and_then(|digest| {
                            opaque_view_types(None, application, digest, types, scratch)
                        })
                        .map(|viewed| viewed.view)
                        .and_then(|view| read(view, &[])),
                    _ => None,
                })
        });
        // A label the application pins is that pin, which *fits* solves it to at every run.
        let pinned = through
            .flatten()
            .and_then(|through| match types.node(through) {
                TypeNode::SignatureApply { pins, .. } => {
                    pins.get(BinderSymbol::Type(entry.label).symbol())
                }
                _ => None,
            });
        // A type member a body binds closed, read as the module's own.
        let bound = || match (tree, entry.through) {
            (Tree::Body { shape, .. }, None) => {
                let (slot, _) = shape.slot(BinderSymbol::Type(entry.label))?;
                match shape.declared_type(slot) {
                    Static::Closed(held) if shape.declarations(slot).is_some() => Some(held),
                    _ => None,
                }
            }
            _ => None,
        };
        let resolved = match pinned.or(closed).or_else(bound) {
            Some(closed) => Resolved::Closed(closed),
            None => Resolved::Class(self.class_of(level, entry, tree)),
        };
        Ok((Resolution { tree, resolved }, at))
    }

    /// The tree, exact signature and a site of the binder `binder` a path's root names, read from
    /// `level`. `None` where the binder's shape is not on the chain.
    fn binder_root(
        &mut self,
        level: usize,
        binder: Binder<'graph>,
    ) -> Read<'graph, Option<(Tree<'graph>, Option<KType>, Site)>> {
        let first = self.chain_root(level);
        let Some(home) = (first..=level)
            .rev()
            .find(|at| std::ptr::eq(self.chain[*at].shape, binder.shape))
        else {
            return Ok(None);
        };
        let shape = binder.shape;
        let statement = shape.position_of(binder.slot).statement_index();
        let Some(site) = statement
            .map(|index| Site::of_node(&shape.body()[index]))
            .or_else(|| shape.body().first().map(Site::of_node))
        else {
            return Ok(None);
        };
        let tree = match shape.position_of(binder.slot) == Position::PARAMETER {
            true => self.leaf_on(Scope::Chain(home), binder.slot)?,
            false => self.slot_tree(Scope::Chain(home), binder.slot, site)?,
        };
        let held = match self.chain[home].binders[binder.slot.index()] {
            DeclaredType::Type(typed) => exact(self.types, typed),
            DeclaredType::Scheme(_) => None,
        };
        Ok(Some((tree, held, site)))
    }

    /// The representative of `entry`'s class: the first path resolved over a tree like `tree`, at
    /// its label, through its application; `entry` itself where there is none yet.
    fn class_of(&mut self, level: usize, entry: &PathEntry<'graph>, tree: Tree<'graph>) -> usize {
        let table = self.table(level);
        let table = &mut self.paths[table];
        // A tree holding a leaf no binder names is like no other.
        match tree.print() {
            Print::Unnamed => entry.level,
            print => *table
                .classes
                .entry((entry.label, entry.through, print))
                .or_insert(entry.level),
        }
    }

    /// `value`, a type a cell of the chain `level` sits on fixed, with each path variable at what
    /// it resolved to: a closed path at its type, any other at its class's variable. `None` where
    /// one is not resolved yet.
    pub(super) fn through_paths(&self, level: usize, value: Parametric) -> Option<Parametric> {
        let (types, scratch) = (self.types, self.scratch);
        let table = &self.paths[self.table(level)];
        let mut unresolved = false;
        let read = read_through(types, scratch, value, Side::Above, &mut |variable| {
            let Variable::Lexical { level: at, .. } = variable else {
                return None;
            };
            let (_, resolution) = table.at(at)?;
            match resolution.map(|resolution| resolution.resolved) {
                None => {
                    unresolved = true;
                    None
                }
                Some(Resolved::Closed(closed)) => Some(Interval::point(closed.into())),
                Some(Resolved::Class(rep)) if rep == at => None,
                Some(Resolved::Class(rep)) => {
                    let (held, _) = table.at(rep).expect("a class's representative is an entry");
                    Some(Interval::point(types.lexical(rep, held.label, KType::ANY)))
                }
            }
        });
        (!unresolved).then_some(read)
    }

    /// `declared`, a type or scheme a cell of the chain `level` sits on fixed over `variables`,
    /// read through its paths' resolutions, beside whether it is then closed. `None` where a path
    /// is not resolved yet, or a scheme reads one the pass cannot substitute: one of a class, or
    /// one beside a name's variable, whose level a substitution would read too.
    fn declared_through(
        &self,
        level: usize,
        declared: DeclaredType<Parametric>,
        variables: &[Located<'graph>],
    ) -> Option<(DeclaredType<Parametric>, bool)> {
        let table = &self.paths[self.table(level)];
        let mut closed = BumpVec::with_capacity_in(variables.len(), self.scratch);
        for variable in variables {
            let point = match variable.source {
                Source::Path { .. } => match table.at(variable.level)?.1?.resolved {
                    Resolved::Closed(point) => Some(point),
                    Resolved::Class(_) => None,
                },
                Source::Type(_) => None,
            };
            closed.push((variable.level, point));
        }
        let all_closed = closed.iter().all(|(_, point)| point.is_some());
        Some(match declared {
            DeclaredType::Type(value) => {
                let value = self.through_paths(level, value)?;
                (DeclaredType::Type(value), all_closed)
            }
            DeclaredType::Scheme(scheme) => {
                if !all_closed {
                    return None;
                }
                let len = closed.iter().map(|(at, _)| at + 1).max().unwrap_or(0);
                let mut bindings = BumpVec::with_capacity_in(len, self.scratch);
                bindings.resize(len, KType::NEVER);
                for (at, point) in closed.iter() {
                    bindings[*at] = point.expect("every path is closed");
                }
                let scheme: Scheme = substitute_levels(self.types, self.scratch, scheme, &bindings);
                (DeclaredType::Scheme(scheme), true)
            }
        })
    }

    /// A type cell of the shape at `level`, read through its paths' resolutions: unknown where one
    /// is not resolved yet, closed where every path it reads is.
    pub(super) fn resolved_type(
        &self,
        level: usize,
        typed: StaticType<'graph>,
    ) -> StaticType<'graph> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        match self.through_paths(level, value) {
            None => Static::Unknown,
            Some(value) => match self.types.concrete(value) {
                Some(closed) => Static::Closed(closed),
                None => Static::Rigid { value, variables },
            },
        }
    }

    /// A callable cell, read from `level`, through its paths' resolutions, as
    /// [`resolved_type`](Self::resolved_type) reads a type cell.
    pub(super) fn resolved_callable(
        &self,
        level: usize,
        typed: StaticCallable<'graph>,
    ) -> StaticCallable<'graph> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        let Some((ktype, closed)) = self.declared_through(level, value.ktype, variables) else {
            return Static::Unknown;
        };
        let registered = match value.registered {
            None => None,
            Some(registered) => match self.registered_through(level, registered, variables) {
                Some((registered, _)) => Some(registered),
                None => return Static::Unknown,
            },
        };
        let value = Callable {
            ktype,
            registered,
            ..value
        };
        match closed {
            true => Static::Closed(Callable {
                ktype: close(self, value.ktype),
                quantifier_map: value.quantifier_map,
                registered: value
                    .registered
                    .map(|registered| closed_registered(self, registered)),
            }),
            false => Static::Rigid { value, variables },
        }
    }

    /// A registration cell, read from `level`, through its paths' resolutions.
    pub(super) fn resolved_registered(
        &self,
        level: usize,
        typed: StaticRegistered<'graph>,
    ) -> StaticRegistered<'graph> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        match self.registered_through(level, value, variables) {
            None => Static::Unknown,
            Some((registered, true)) => Static::Closed(closed_registered(self, registered)),
            Some((value, false)) => Static::Rigid { value, variables },
        }
    }

    fn registered_through(
        &self,
        level: usize,
        registered: Registered<'graph>,
        variables: &[Located<'graph>],
    ) -> Option<(Registered<'graph>, bool)> {
        let (shape, closed) = self.declared_through(level, registered.shape, variables)?;
        Some((
            Registered {
                shape,
                ..registered
            },
            closed,
        ))
    }

    /// The static type of the callable body `body`, written in the shape at `level`: a point at its
    /// function type, or its scheme, where the load typed it, `[Never, Any]` elsewhere.
    pub(super) fn callable(&self, level: usize, body: &BodyShape<'graph>) -> Bound {
        let ktype = match self.resolved_callable(level, body.callable_type()) {
            Static::Closed(callable) => callable.ktype.into(),
            Static::Rigid {
                value: callable, ..
            } => callable.ktype,
            Static::Unknown => return DeclaredType::Type(unknown()),
        };
        ktype.map(Interval::point)
    }

    /// Whether the shape at `level` reaches a path of the class each path among `variables` — a
    /// cell's — resolved to.
    pub(super) fn reaches(&self, level: usize, variables: &[Located<'graph>]) -> bool {
        let table = &self.paths[self.table(level)];
        variables.iter().all(|variable| match variable.source {
            Source::Type(_) => true,
            Source::Path { .. } => match table.at(variable.level).and_then(|(_, held)| held) {
                Some(Resolution {
                    resolved: Resolved::Class(rep),
                    ..
                }) => self.reachable(level, rep).is_some(),
                // A closed path needs no root, and an unresolved one reads as unknown.
                _ => true,
            },
        })
    }

    /// Where the shape at `level` reads the root of `entry`, a path through a binder on its chain:
    /// the binder's slot stepped in through every shape between — a hop per block, and the builder
    /// capture reading it per callable or module. `None` where a shape between captures no such
    /// thing, or the root is written in place.
    fn path_coordinate(&self, level: usize, entry: &PathEntry<'graph>) -> Option<Coordinate> {
        let Root::Binder(binder) = entry.root else {
            return None;
        };
        let first = self.chain_root(level);
        let home = (first..=level)
            .rev()
            .find(|at| std::ptr::eq(self.chain[*at].shape, binder.shape))?;
        let mut coordinate = Coordinate::Activation {
            hops: 0,
            target: Target::Local(binder.slot),
        };
        for inner in home + 1..=level {
            let shape = self.chain[inner].shape;
            coordinate = match shape.kind() {
                ShapeKind::Block => coordinate.through_block(),
                ShapeKind::Callable | ShapeKind::Module => {
                    let index = (shape.captures().iter())
                        .position(|spec| spec.source == CaptureSource::Read(coordinate))?;
                    Coordinate::Activation {
                        hops: 0,
                        target: Target::Capture(CaptureSlot(index as u32)),
                    }
                }
                ShapeKind::Program | ShapeKind::Code => return None,
            };
        }
        Some(coordinate)
    }

    /// A path of the class whose variable is at `rep` the shape at `level` reaches, beside where it
    /// reads the path's root: one whose application names no path variable. `None` where `rep` is
    /// a name level, or no path of the class is reachable.
    pub(super) fn reachable(
        &self,
        level: usize,
        rep: usize,
    ) -> Option<(&'graph PathEntry<'graph>, Coordinate)> {
        let table = &self.paths[self.table(level)];
        table.at(rep)?;
        let from = table.members.partition_point(|(class, _)| *class < rep);
        let to = table.members.partition_point(|(class, _)| *class <= rep);
        table.members[from..to].iter().find_map(|(_, member)| {
            let (entry, _) = table.at(*member)?;
            if !self.locatable(level, entry) {
                return None;
            }
            Some((entry, self.path_coordinate(level, entry)?))
        })
    }

    /// Whether the application `entry` is read through names no path variable, so the shape at
    /// `level` can locate what it names.
    fn locatable(&self, level: usize, entry: &PathEntry<'graph>) -> bool {
        let Some(through) = entry.through else {
            return true;
        };
        let table = &self.paths[self.table(level)];
        let mut paths = false;
        read_through(
            self.types,
            self.scratch,
            through,
            Side::Above,
            &mut |variable| {
                if let Variable::Lexical { level, .. } = variable {
                    paths |= table.at(level).is_some();
                }
                None
            },
        );
        !paths
    }

    /// Whether `level` is a path level of the chain the shape at `from` sits on.
    pub(super) fn is_path(&self, from: usize, level: usize) -> bool {
        self.paths[self.table(from)].at(level).is_some()
    }

    /// `typed`, a static type read in the shape at `level`, with each path variable no path of
    /// whose class that shape reaches read through its bounds: an end at a time, from the side it
    /// bounds.
    pub(super) fn sourced(&self, level: usize, typed: Interval) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let mut unreached = |variable: Variable| {
            let Variable::Lexical { level: at, .. } = variable else {
                return None;
            };
            (self.is_path(level, at) && self.reachable(level, at).is_none()).then(|| Interval {
                lower: KType::NEVER.into(),
                upper: KType::ANY.into(),
            })
        };
        Interval {
            lower: read_through(types, scratch, typed.lower, Side::Below, &mut unreached),
            upper: read_through(types, scratch, typed.upper, Side::Above, &mut unreached),
        }
    }

    /// [`sourced`](Self::sourced) over a binder's static type: a scheme as it is.
    pub(super) fn sourced_bound(&self, level: usize, bound: Bound) -> Bound {
        match bound {
            DeclaredType::Type(typed) => DeclaredType::Type(self.sourced(level, typed)),
            scheme => scheme,
        }
    }

    /// The signature `upper`, the static upper end of a module the root `root` names read through
    /// `chain` in the shape at `level`, applies, beside its pins and each head parameter it leaves
    /// unpinned pinned to the path through it, read through that application. `None` where `upper`
    /// is no application with a parameter unpinned, or one of its paths is out of reach.
    ///
    /// A member is read under these pins, and nothing else: a pin over a variable read through its
    /// bounds is no application above the module, since two pins of one parameter are unordered,
    /// so no static type holds one.
    fn pinned(
        &mut self,
        level: usize,
        root: Root<'graph>,
        chain: &[ValueSymbol],
        upper: Parametric,
    ) -> Read<'graph, Option<(KType, Pins<'p>)>> {
        let types = self.types;
        let (signature, pinned) = match types.node(upper) {
            TypeNode::Signature { .. } => match types.concrete(upper) {
                Some(signature) => (signature, None),
                None => return Ok(None),
            },
            TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
            _ => return Ok(None),
        };
        let Some(schema) = crate::elaborate::schema_of(signature, types) else {
            return Ok(None);
        };
        let mut pins = BumpVec::new_in(self.scratch);
        pins.extend(pinned.iter().flat_map(|pins| pins.iter()));
        let mut changed = false;
        for (name, _) in schema.parameters.iter() {
            let parameter = BinderSymbol::Type(*name);
            if pins.iter().any(|(pin, _)| *pin == parameter) {
                continue;
            }
            let entry = self.path(level, root, chain, *name, Some(upper));
            let pin = match self.resolve_path(level, entry)? {
                Resolved::Closed(closed) => closed.into(),
                // A path this shape cannot read the root of — across a quote's code — names no
                // type here.
                Resolved::Class(rep) if self.reachable(level, rep).is_none() => return Ok(None),
                Resolved::Class(rep) => types.lexical(rep, *name, KType::ANY),
            };
            pins.push((parameter, pin));
            changed = true;
        }
        Ok(changed.then_some((signature, pins)))
    }

    /// The member `name` of the module `operand`, a part of the shape at `level` whose static upper
    /// end is `upper`: off `upper`, and where it leaves a head parameter the member names unpinned
    /// and `operand` is a name chain, under the [pins](Self::pinned) its paths give. So `m.zero`
    /// and `m.succ` read one path for `m`'s `Carrier`.
    pub(super) fn member_read(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        upper: Parametric,
        name: BinderSymbol,
    ) -> Read<'graph, Option<SignatureMember>> {
        let (types, scratch) = (self.types, self.scratch);
        let member = signature_member(types, scratch, upper, name);
        if !matches!(
            member,
            Some(SignatureMember {
                unpinned: Some(_),
                ..
            })
        ) {
            return Ok(member);
        }
        // An operand no name chain reaches roots its paths where it is written.
        let (root, chain) = match self.name_chain(level, operand) {
            Some((binder, chain)) => (Root::Binder(binder), chain),
            None => (
                Root::Inline(Site::of(operand)),
                BumpVec::new_in(self.scratch),
            ),
        };
        Ok(match self.pinned(level, root, &chain, upper)? {
            Some((signature, pins)) => applied_member(types, scratch, signature, &pins, name),
            None => member,
        })
    }

    /// `typed`, the static type of the read of member `label` through `operand` in the shape at
    /// `level`, its rule having read it off the operand's static type: where that leaves a head
    /// parameter the member names unpinned, at most the member [read through its
    /// paths](Self::member_read). A module member whose type pins a parameter to a path stays as
    /// its rule read it.
    pub(super) fn read_through_paths(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        label: BinderSymbol,
        typed: Interval,
    ) -> Read<'graph, Interval> {
        let (types, scratch) = (self.types, self.scratch);
        if !matches!(label, BinderSymbol::Value(_)) || typed.upper == KType::NEVER.into() {
            return Ok(typed);
        }
        let site = Site::of(operand);
        let Some((_, operand_type)) = (self.chain[level].parts.iter().rev())
            .find(|(at, _)| *at == site)
            .copied()
        else {
            return Ok(typed);
        };
        let upper = operand_type.upper;
        if !matches!(
            signature_member(types, scratch, upper, label),
            Some(SignatureMember {
                unpinned: Some(_),
                ..
            })
        ) {
            return Ok(typed);
        }
        Ok(match self.member_read(level, operand, upper, label)? {
            Some(SignatureMember {
                declared: DeclaredType::Type(declared),
                unpinned: None,
            }) if !matches!(types.node(declared), TypeNode::SignatureApply { .. })
                || types.concrete(declared).is_some() =>
            {
                under(declared)
            }
            _ => typed,
        })
    }

    /// The binder the name chain `part` of the shape at `level` is rooted at, beside the value
    /// members it reads through it: `m`, `m.inner`. `None` for any other part.
    pub(super) fn name_chain(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<(Binder<'graph>, BumpVec<'p, ValueSymbol>)> {
        let shape = self.chain[level].shape;
        match super::of_part(shape, part) {
            super::Form::Name(name) => {
                let mention = shape.mention(Site::of(name))?;
                let (home, slot) = self.landed_slot(level, mention.coordinate)?;
                let binder = Binder {
                    shape: self.chain[home].shape,
                    slot,
                };
                Some((binder, BumpVec::new_in(self.scratch)))
            }
            super::Form::Member {
                operand,
                label: BinderSymbol::Value(label),
                ..
            } => {
                let (binder, mut chain) = self.name_chain(level, operand)?;
                chain.push(label);
                Some((binder, chain))
            }
            _ => None,
        }
    }
}

/// `typed`'s type where it is exact and closed.
fn exact(types: &crate::values::Types<'_, '_>, typed: Interval) -> Option<KType> {
    typed
        .is_exact()
        .then(|| types.concrete(typed.upper))
        .flatten()
}

/// `declared`, read through paths that all resolved closed, as the closed type or scheme it is.
fn close(pass: &Pass<'_, '_, '_>, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
    match declared {
        DeclaredType::Type(value) => DeclaredType::Type(
            pass.types
                .concrete(value)
                .expect("a type over closed paths alone is closed"),
        ),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    }
}

fn closed_registered<'graph>(
    pass: &Pass<'_, '_, '_>,
    registered: Registered<'graph>,
) -> Registered<'graph, KType> {
    Registered {
        shape: close(pass, registered.shape),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
    }
}

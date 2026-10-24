//! The walk's paths ([design/modules.md § Comparing two paths](../../../design/modules.md#comparing-two-paths)):
//! each path entry of a chain resolved to the type it is at load, every fact the type half fixes
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
//! The walk resolves the paths a unit's statements spell in the unit's first step, before its
//! declarations, an inline root typed there. A module's static type whose upper end leaves a head parameter
//! unpinned is pinned to the path through it, a path read through that application, so two member
//! reads through one module share a type.
//!
//! A path variable reaches a static type only where a path of its class is **reachable**: its root
//! a binder on the chain the reading shape reads through its blocks and builder captures. The pass
//! adds no capture for a path, so a static type read across a callable or module boundary reads
//! each variable no path of whose class is reachable through its bounds, and every path variable a
//! use records is located at a reachable root.

use crate::elaborate::{SignatureMember, applied_member, path_binding, signature_member};
use crate::memory::{BumpBackedMap, BumpVec, bump_hash_table, bump_table, collect};
use crate::parse::ExpressionPart;
use crate::scope::{
    Binder, BodyShape, Callable, CaptureSlot, CaptureSource, Coordinate, Identity, Leaf, PathEntry,
    Position, Registered, Resolution, Resolved, Root, Segment, ShapeError, ShapeKind, Site, Source,
    SpelledRoot, Static, StaticCallable, StaticRegistered, StaticType, Store, Target, Tree,
    Variable as Located, part_of, path_spelling, source_of,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, Scheme, Side, TypeNode, Variable, read_through,
    substitute_levels,
};

use super::reads::Part;
use super::trees::Scope;
use super::{Bound, Pass, Tag, under, unknown};

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// The path entries the walk holds for one chain, found by root, member chain and label: each
/// entry's index among them, hashed by [`PathEntry::hash_of`].
type Found<'p> = crate::memory::BumpHashTable<'p, usize>;

/// An application's pins, by parameter.
type Pins<'p> = BumpVec<'p, (BinderSymbol, Parametric)>;

/// One chain's path entries, each beside its resolution once the walk resolved it and the unit
/// that did: each spelled identity at the level the shape numbers it, then each the walk mints
/// past them.
pub(super) struct ChainPaths<'p, 'o, 'graph> {
    root: &'graph BodyShape<'graph>,
    /// The first path level: past every name level of the chain.
    base: usize,
    /// Every entry, by its level less `base`.
    entries: BumpVec<'p, Option<Entry<'o>>>,
    /// Each entry's index in `entries`, by root, chain and label.
    found: Found<'p>,
    /// Each class's representative's level, by the label, application and tree identity its paths
    /// share.
    classes: BumpBackedMap<'p, (TypeSymbol, Option<Parametric>, Identity), usize>,
    /// Each path resolved to a class, by the class's representative's level, then its own.
    members: BumpVec<'p, (usize, usize)>,
    /// Each entry being resolved, by level.
    resolving: BumpVec<'p, usize>,
}

/// One entry of a chain's paths.
#[derive(Clone, Copy)]
struct Entry<'o> {
    entry: PathEntry<'o>,
    /// Whether the entry's `through` is read: a spelled identity's is read off its root's declared
    /// type once the walk first needs it.
    through: bool,
    resolution: Option<(Resolution<'o>, Tag)>,
}

impl<'o> ChainPaths<'_, 'o, '_> {
    /// The entry at `level`, beside its resolution: `None` where `level` is a name level.
    fn at(&self, level: usize) -> Option<(PathEntry<'o>, Option<(Resolution<'o>, Tag)>)> {
        let held = (*self.entries.get(level.checked_sub(self.base)?)?)?;
        Some((held.entry, held.resolution))
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Open the path table of the chain `root` roots, holding each identity a type of the chain
    /// spells at its level.
    pub(super) fn open_paths(&mut self, root: &'graph BodyShape<'graph>) {
        debug_assert!(
            (self.chain.last())
                .is_some_and(|at| std::ptr::eq(at.shape, root) && at.paths == self.paths.len()),
            "a chain's root opens its table as it is pushed"
        );
        let (shapes, scratch) = (self.shapes, self.scratch);
        let segment = root.segment();
        let base = shapes.spelled_base(segment);
        let mut entries: BumpVec<'p, Option<Entry<'o>>> =
            BumpVec::with_capacity_in(shapes.path_base(segment) - base, scratch);
        entries.resize(shapes.path_base(segment) - base, None);
        let mut found: Found<'p> = bump_hash_table(scratch);
        // The chain's bodies are the root's subtree, short of a quote's code: it roots its own.
        let mut pending = BumpVec::new_in(scratch);
        pending.push(root);
        while let Some(shape) = pending.pop() {
            pending.extend(
                (shape.nested_shapes().iter())
                    .map(|(_, nested)| *nested)
                    .filter(|nested| nested.kind() != ShapeKind::Code),
            );
            for (index, spelled) in shape.spelled().iter().enumerate() {
                let (Some(id), Some(level)) =
                    (spelled.id, shapes.path_level(shape.segment(), index))
                else {
                    continue;
                };
                let at = level - base;
                if entries[at].is_some() {
                    continue;
                }
                entries[at] = Some(Entry {
                    entry: PathEntry {
                        root: id.root,
                        chain: id.chain,
                        label: id.label,
                        level,
                        through: None,
                    },
                    through: false,
                    resolution: None,
                });
                let hash = PathEntry::hash_of(id.root, id.chain, id.label);
                found.insert_unique(hash, at, |held: &usize| {
                    let held = entries[*held].expect("an indexed entry is held").entry;
                    PathEntry::hash_of(held.root, held.chain, held.label)
                });
            }
        }
        let table: ChainPaths<'p, 'o, 'graph> = ChainPaths {
            root,
            base,
            entries,
            found,
            classes: bump_table(scratch),
            members: BumpVec::new_in(scratch),
            resolving: BumpVec::new_in(scratch),
        };
        self.paths.push(table);
    }

    /// Close the path table of the chain `root` roots: each resolution rests in the record of the
    /// unit that made it.
    pub(super) fn close_paths(&mut self, root: &'graph BodyShape<'graph>) {
        let table = self.paths.pop().expect("a chain's paths are open");
        debug_assert!(std::ptr::eq(table.root, root));
    }

    /// Put `entry`, a path of the chain `root` roots, back into its table beside `resolution`,
    /// which the absorbed unit `tag` made.
    pub(super) fn absorb_path(
        &mut self,
        root: Segment,
        entry: PathEntry<'o>,
        resolution: Resolution<'o>,
        tag: Tag,
    ) {
        let Some(at) = (self.paths.iter()).rposition(|table| table.root.segment() == root) else {
            return;
        };
        let table = &mut self.paths[at];
        let index = entry.level - table.base;
        if table.entries.len() <= index {
            table.entries.resize(index + 1, None);
        }
        let fresh = table.entries[index].is_none();
        table.entries[index] = Some(Entry {
            entry,
            through: true,
            resolution: Some((resolution, tag)),
        });
        if fresh {
            let entries = &table.entries;
            let hash = PathEntry::hash_of(entry.root, entry.chain, entry.label);
            table.found.insert_unique(hash, index, |held: &usize| {
                let held = entries[*held].expect("an indexed entry is held").entry;
                PathEntry::hash_of(held.root, held.chain, held.label)
            });
        }
        if let Resolved::Class(rep) = resolution.resolved {
            let identity = resolution.tree.identity();
            if !matches!(identity, Identity::Unnamed) {
                let key = (entry.label, entry.through, identity);
                table.classes.entry(key).or_insert(rep);
            }
            let at = (table.members).partition_point(|member| *member < (rep, entry.level));
            table.members.insert(at, (rep, entry.level));
        }
    }

    /// The resolution of the path at `level` of the chain `root` roots, beside the unit that made
    /// it, where the walk has it.
    pub(super) fn resolved_at(&self, root: Segment, level: usize) -> Option<(Resolution<'o>, Tag)> {
        let table = (self.paths.iter())
            .rev()
            .find(|table| table.root.segment() == root)?;
        table.at(level)?.1
    }

    /// The entry at `path_level` of the chain the shape at `level` sits on, its application read.
    pub(super) fn entry_at(&mut self, level: usize, path_level: usize) -> Option<PathEntry<'o>> {
        let table = self.table(level);
        let index = path_level.checked_sub(self.paths[table].base)?;
        self.read_through_of(level, index);
        Some(
            self.paths[self.table(level)]
                .entries
                .get(index)
                .copied()??
                .entry,
        )
    }

    /// The chain level of the root of the chain `level` sits on.
    pub(super) fn chain_root(&self, level: usize) -> usize {
        (0..=level)
            .rev()
            .find(|at| self.chain[*at].roots_chain)
            .expect("every chain has a root")
    }

    /// The index in `paths` of the table of the chain `level` sits on.
    fn table(&self, level: usize) -> usize {
        let table = self.chain[level].paths;
        debug_assert!(
            table < self.paths.len(),
            "a chain's paths are open while it is typed"
        );
        table
    }

    /// The segment of the body rooting the chain `level` sits on.
    pub(super) fn chain_root_segment(&self, level: usize) -> Segment {
        self.paths[self.table(level)].root.segment()
    }

    /// The path entry of the chain `level` sits on for `root`, `chain` and `label`, read through
    /// `through`: the one held already, or a new one at the chain's next path level.
    fn path(
        &mut self,
        level: usize,
        root: Root,
        chain: &[ValueSymbol],
        label: TypeSymbol,
        through: Option<Parametric>,
    ) -> PathEntry<'o> {
        let table = self.table(level);
        let hash = PathEntry::hash_of(root, chain, label);
        // A spelled identity's application is read before it is compared.
        let mut candidates = BumpVec::new_in(self.scratch);
        candidates.extend((self.paths[table].found).iter_hash(hash).copied());
        for index in candidates.iter() {
            self.read_through_of(level, *index);
        }
        let table = &mut self.paths[table];
        let held = candidates.iter().copied().find(|at| {
            table.entries[*at].is_some_and(|held| {
                held.entry.is(root, chain, label) && held.entry.through == through
            })
        });
        if let Some(at) = held {
            return table.entries[at].expect("an indexed entry is held").entry;
        }
        let index = table.entries.len();
        let entry = PathEntry {
            root,
            chain: collect(self.lay, chain.iter().copied()),
            label,
            level: table.base + index,
            through,
        };
        table.entries.push(Some(Entry {
            entry,
            through: true,
            resolution: None,
        }));
        let entries = &table.entries;
        table.found.insert_unique(hash, index, |held: &usize| {
            let held = entries[*held].expect("an indexed entry is held").entry;
            PathEntry::hash_of(held.root, held.chain, held.label)
        });
        entry
    }

    /// Read the application of the entry at `index` of the chain `level` sits on, where it is a
    /// spelled identity's not read yet: its root's declared type, where it leaves the label an
    /// unpinned head parameter.
    fn read_through_of(&mut self, level: usize, index: usize) {
        let table = self.table(level);
        let Some(held) = self.paths[table].entries[index] else {
            return;
        };
        if held.through {
            return;
        }
        let entry = held.entry;
        let declared = match entry.root {
            Root::Binder(binder) => self.declared_root(binder),
            Root::Inline { .. } => None,
        };
        let through = declared.and_then(|declared| {
            crate::elaborate::declared_through(
                self.types,
                self.scratch,
                declared,
                entry.chain,
                entry.label,
            )
        });
        let slot = self.paths[table].entries[index]
            .as_mut()
            .expect("the entry is held");
        slot.entry.through = through;
        slot.through = true;
    }

    /// The declared type of the value binder `binder`: a parameter's, as its body reads it, or an
    /// annotated binder's annotation.
    fn declared_root(&self, binder: Binder) -> Option<Parametric> {
        let shape = self.shape_at(binder.body);
        if shape.kind() == ShapeKind::Callable
            && shape.position_of(binder.slot) == Position::PARAMETER
        {
            return self.body_type(
                shape,
                Part::Parameter(shape.slot_name(binder.slot).symbol()),
            );
        }
        let annotation = shape.annotation(binder.slot)?;
        match self.type_cell(shape, shape.site(annotation)) {
            Static::Closed(handle) => Some(handle.into()),
            Static::Rigid { value, .. } => Some(value),
            Static::Unknown => None,
        }
    }

    /// Resolve the path `shape`'s spelling at `index` of [`BodyShape::spelled`] names, the shape at
    /// `level`, and record where the shape reads it for the run: step one of a unit.
    pub(super) fn resolve_spelled(&mut self, level: usize, index: usize) -> Read<'o, ()> {
        let shape = self.chain[level].shape;
        let spelled = shape.spelled()[index];
        let Some(path_level) = self.shapes.path_level(shape.segment(), index) else {
            return Ok(());
        };
        let table = self.table(level);
        let at = path_level - self.paths[table].base;
        self.read_through_of(level, at);
        let entry = (self.paths[self.table(level)].entries[at])
            .expect("a spelled identity has its entry")
            .entry;
        let (ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node)) =
            spelled.part
        else {
            unreachable!("the builder records a path's node");
        };
        let spelling = path_spelling(node.reference(), self.scratch).map(|spelling| spelling.root);
        // A root written in place is typed where its path resolves, its own types first.
        if let Some(SpelledRoot::Inline(written)) = spelling {
            let holds = |site: Site| match written {
                ExpressionPart::Expression(node) => {
                    part_of(shape, node.reference(), site).is_some()
                }
                _ => false,
            };
            // Its own types sit in its statement, so among its unit's, in program order.
            let unit = shape.holds(shape.unit_of_statement(spelled.statement as usize));
            for at in unit.expressions.iter() {
                let expression = &shape.type_expressions()[*at as usize];
                if holds(expression.site) {
                    self.type_expression(level, expression)?;
                }
            }
        }
        self.open_path = None;
        let resolved = self.resolve_path(level, entry)?;
        // A path a type reads through a module built in place that does not resolve closed refuses
        // the load, since the run builds no module there; a type binder reads it as unknown.
        // A type binder binds at its statement's unit, so only that unit's slots may declare.
        let statement = spelled.statement as usize;
        let declaring =
            (shape.holds(shape.unit_of_statement(statement)).slots.iter()).any(|slot| {
                (shape.declarations(*slot))
                    .is_some_and(|node| std::ptr::eq(node, &shape.body()[statement]))
            });
        match (entry.root, resolved, self.open_path.take()) {
            (Root::Inline { .. }, Resolved::Class(_), Some(at)) if !declaring => {
                Err(ShapeError::OpenPathInType { at })
            }
            _ => Ok(()),
        }
    }

    /// What `entry`, a path of the chain the shape at `level` sits on, resolves to, resolving it
    /// now where the walk has not: a path's root written in place is typed in that shape.
    pub(super) fn resolve_path(
        &mut self,
        level: usize,
        entry: PathEntry<'o>,
    ) -> Read<'o, Resolved> {
        let table = self.table(level);
        if let Some((_, Some((resolution, tag)))) = self.paths[table].at(entry.level) {
            self.gather(tag);
            return Ok(resolution.resolved);
        }
        // A path met while it is being resolved stands for itself.
        if self.paths[table].resolving.contains(&entry.level) {
            return Ok(Resolved::Class(entry.level));
        }
        self.paths[table].resolving.push(entry.level);
        let resolution = self.resolve_entry(level, entry);
        self.paths[table].resolving.pop();
        let (resolution, at) = resolution?;
        let index = entry.level - self.paths[table].base;
        if let (Root::Inline { .. }, Resolved::Class(_), Some(at)) =
            (entry.root, resolution.resolved, at)
        {
            self.open_path = Some(at);
        }
        let tag = self.innermost();
        let root = self.paths[table].root.segment();
        let table = &mut self.paths[table];
        if let Some(held) = table.entries[index].as_mut() {
            held.resolution = Some((resolution, tag));
        }
        if let Resolved::Class(rep) = resolution.resolved {
            let at = table
                .members
                .partition_point(|member| *member < (rep, entry.level));
            table.members.insert(at, (rep, entry.level));
        }
        if !self.unfilled {
            self.units[tag.0 as usize]
                .paths
                .push(((root, entry.level), (entry, resolution)));
        }
        Ok(resolution.resolved)
    }

    /// `entry`'s resolution, read from `level`, beside where its root is written where it is
    /// written in place.
    fn resolve_entry(
        &mut self,
        level: usize,
        entry: PathEntry<'o>,
    ) -> Read<'o, (Resolution<'o>, Option<crate::source::SourceRef>)> {
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
            Root::Inline { site, statement } => {
                let shape = self.chain[level].shape;
                let statement = statement as usize;
                let node = &shape.body()[statement];
                match part_of(shape, node, site) {
                    Some(part) => {
                        let was = std::mem::replace(&mut self.chain[level].statement, statement);
                        let typed = match self.part_type(level, site) {
                            Some(typed) => typed,
                            None => self.part(level, part)?,
                        };
                        let tree = self.tree(level, part)?;
                        self.chain[level].statement = was;
                        let at =
                            (source_of(shape, node, site)).expect("the part lies in its statement");
                        (tree, exact(types, typed), site, Some(at))
                    }
                    None => (Tree::Leaf(Leaf::Unnamed(site)), None, site, None),
                }
            }
        };
        let tree =
            (self.read_chain(root, entry.chain, site)?).unwrap_or(Tree::Leaf(Leaf::Unnamed(site)));
        let through = match entry.through {
            None => Some(None),
            Some(through) => types.concrete(through).map(Some),
        };
        // The signature of the module the root names, or the chain reaches, where the load knows
        // it exactly: the path is then what the run reads off it.
        let view = match tree {
            Tree::View { .. } => self.view_of(tree, site)?.map(|viewed| viewed.view),
            _ => None,
        };
        let closed = through.and_then(|through| {
            let read = |held: KType, chain: &[ValueSymbol]| {
                path_binding(types, scratch, held, chain, entry.label, through)
            };
            held.and_then(|held| read(held, entry.chain))
                .or_else(|| view.and_then(|view| read(view, &[])))
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
            (Tree::Body { body, .. }, None) => {
                let shape = self.shape_at(body);
                let (slot, _) = shape.slot(BinderSymbol::Type(entry.label))?;
                match self.declared_cell(shape, slot) {
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
        binder: Binder,
    ) -> Read<'o, Option<(Tree<'o>, Option<KType>, Site)>> {
        let first = self.chain_root(level);
        let Some(home) = (first..=level)
            .rev()
            .find(|at| self.chain[*at].shape.segment() == binder.body)
        else {
            return Ok(None);
        };
        let shape = self.chain[home].shape;
        let statement = shape.position_of(binder.slot).statement_index();
        let Some(site) = statement
            .map(|index| shape.site_of_node(&shape.body()[index]))
            .or_else(|| shape.body().first().map(|node| shape.site_of_node(node)))
        else {
            return Ok(None);
        };
        let tree = match shape.position_of(binder.slot) == Position::PARAMETER {
            true => self.leaf_on(Scope::Chain(home), binder.slot)?,
            false => self.slot_tree(Scope::Chain(home), binder.slot, site)?,
        };
        let held = match self.binder_at(home, binder.slot) {
            DeclaredType::Type(typed) => exact(self.types, typed),
            DeclaredType::Scheme(_) => None,
        };
        Ok(Some((tree, held, site)))
    }

    /// The representative of `entry`'s class: the first path resolved over a tree like `tree`, at
    /// its label, through its application; `entry` itself where there is none yet.
    fn class_of(&mut self, level: usize, entry: PathEntry<'o>, tree: Tree<'o>) -> usize {
        let table = self.table(level);
        let table = &mut self.paths[table];
        // A tree holding a leaf no binder names is like no other.
        match tree.identity() {
            Identity::Unnamed => entry.level,
            identity => *table
                .classes
                .entry((entry.label, entry.through, identity))
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
            if let Some((_, tag)) = resolution {
                self.gather(tag);
            }
            match resolution.map(|(resolution, _)| resolution.resolved) {
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
        variables: &[Located],
    ) -> Option<(DeclaredType<Parametric>, bool)> {
        let table = &self.paths[self.table(level)];
        let mut closed = BumpVec::with_capacity_in(variables.len(), self.scratch);
        for variable in variables {
            let point = match variable.source {
                Source::Path { .. } => match self.noted(table.at(variable.level)?.1?).resolved {
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
    pub(super) fn resolved_type(&self, level: usize, typed: StaticType<'o>) -> StaticType<'o> {
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
        typed: StaticCallable<'o>,
    ) -> StaticCallable<'o> {
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
                ktype: self.types.concrete_declared(value.ktype).expect(CLOSED),
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
        typed: StaticRegistered<'o>,
    ) -> StaticRegistered<'o> {
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
        registered: Registered<'o>,
        variables: &[Located],
    ) -> Option<(Registered<'o>, bool)> {
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
        let ktype = match self.resolved_callable(level, self.callable_cell(body)) {
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
    pub(super) fn reaches(&self, level: usize, variables: &[Located]) -> bool {
        let table = &self.paths[self.table(level)];
        variables.iter().all(|variable| match variable.source {
            Source::Type(_) => true,
            Source::Path { .. } => match table
                .at(variable.level)
                .and_then(|(_, held)| held)
                .map(|held| self.noted(held))
            {
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
    fn path_coordinate(&self, level: usize, entry: PathEntry<'o>) -> Option<Coordinate> {
        let Root::Binder(binder) = entry.root else {
            return None;
        };
        let first = self.chain_root(level);
        let home = (first..=level)
            .rev()
            .find(|at| self.chain[*at].shape.segment() == binder.body)?;
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
    ) -> Option<(PathEntry<'o>, Coordinate)> {
        let table = &self.paths[self.table(level)];
        table.at(rep)?;
        let from = table.members.partition_point(|(class, _)| *class < rep);
        let to = table.members.partition_point(|(class, _)| *class <= rep);
        table.members[from..to].iter().find_map(|(_, member)| {
            let (entry, resolution) = table.at(*member)?;
            if let Some((_, tag)) = resolution {
                self.gather(tag);
            }
            if !self.locatable(level, entry) {
                return None;
            }
            Some((entry, self.path_coordinate(level, entry)?))
        })
    }

    /// Whether the application `entry` is read through names no path variable, so the shape at
    /// `level` can locate what it names.
    fn locatable(&self, level: usize, entry: PathEntry<'o>) -> bool {
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

    /// `resolution`, the frontier of the unit that made it gathered.
    fn noted(&self, (resolution, tag): (Resolution<'o>, Tag)) -> Resolution<'o> {
        self.gather(tag);
        resolution
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
        root: Root,
        chain: &[ValueSymbol],
        upper: Parametric,
    ) -> Read<'o, Option<(KType, Pins<'p>)>> {
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
    ) -> Read<'o, Option<SignatureMember>> {
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
            None => {
                let site = self.site(level, operand);
                let statement = self.statement_holding(level, site) as u32;
                (
                    Root::Inline { site, statement },
                    BumpVec::new_in(self.scratch),
                )
            }
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
    ) -> Read<'o, Interval> {
        let (types, scratch) = (self.types, self.scratch);
        if !matches!(label, BinderSymbol::Value(_)) || typed.upper == KType::NEVER.into() {
            return Ok(typed);
        }
        let site = self.site(level, operand);
        let Some(operand_type) = self.part_type(level, site) else {
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
    ) -> Option<(Binder, BumpVec<'p, ValueSymbol>)> {
        let shape = self.chain[level].shape;
        match super::of_part(shape, part) {
            super::Form::Name(name) => {
                let mention = shape.mention(shape.site(name))?;
                let (home, slot) = self.landed_slot(level, mention.coordinate)?;
                let binder = Binder {
                    body: self.chain[home].shape.segment(),
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
pub(super) fn exact(
    types: &crate::values::Chain<'_, '_, '_, '_>,
    typed: Interval,
) -> Option<KType> {
    typed
        .is_exact()
        .then(|| types.concrete(typed.upper))
        .flatten()
}

/// Why a declared type read through paths that all resolved closed is concrete: each path reads a
/// closed type, so no variable is left to reach.
const CLOSED: &str = "a type over closed paths alone is closed";

fn closed_registered<'o, S: Store<'o>>(
    pass: &Pass<'_, 'o, '_, '_, '_, '_, '_, S>,
    registered: Registered<'o>,
) -> Registered<'o, KType> {
    Registered {
        shape: pass
            .types
            .concrete_declared(registered.shape)
            .expect(CLOSED),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
    }
}

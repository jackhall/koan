//! The walk's paths ([design/modules.md § Comparing two paths](../../../design/modules.md#comparing-two-paths)):
//! each path entry of a chain resolved to the type it is at load, every fact the type half fixes
//! read through those resolutions, and a module's static type pinned through its paths.
//!
//! A path **resolves** off the content tree of the module its root names, walked through its member
//! chain. It is **closed** — one type at every run — where the load knows that module's signature
//! exactly, from the root's exact static type or a closed opaque view's tree, and reads the path
//! off it as the run does ([`path_binding`]), where its application pins its label, or where it
//! reads a type member a body binds closed.
//! Any other path is its **class**'s variable, a [`TypeNode::Path`] named by the class's content:
//! its label, its application and its tree's identity, so every walk names a class alike whatever
//! it resolved before. A path holding a leaf no binder names is alone in its class. An entry is
//! named by content too ([`PathName`]): a spelled path by the level the shape numbers it at, a path
//! the walk mints by its root, member chain, label and application. So a chain's table opens empty.
//! A path written in a type through a module built in place that does not resolve closed refuses
//! the load, since the run builds no module there to read it from.
//!
//! The walk resolves the paths a unit's statements spell in the unit's first step, before its
//! declarations, an inline root typed there. A module's static type whose upper end leaves a head
//! parameter unpinned is pinned to the path through it, a path read through that application, so
//! two member reads through one module share a type. A value the run retypes by its fit at such a
//! signature — a parameter, an annotated `LET`, a `LET` binding a call, ascription or `EVAL`
//! whole, a module member read through a chain of module members, an arm's `it` — is typed
//! exactly at that pinned application, rooted at its binder; one written in place, or read through
//! a record's field, stays at most its signature. A path written through a binder exact at an
//! application pinning its label reads that pin.
//!
//! A path variable reaches a static type only where its class is **reachable**, by the class's
//! content alone ([`reachable`](Pass::reachable)): a binder on the chain the reading shape reads
//! through its blocks and builder captures holds, at some member chain, a module of the class's
//! tree — the leaf binder the tree reads through, a module binder whose content tree it is, or a
//! member of a module body one holds. The pass adds no capture for a path, so a static type read
//! across a callable or module boundary reads each variable whose class is not reachable through
//! its bounds, and every path variable a use records is located at a path through such a binder,
//! minted where the walk holds none.

use crate::elaborate::judge;
use crate::elaborate::{SignatureMember, applied_member, path_binding, signature_member};
use crate::memory::{BumpBackedMap, BumpVec, bump_table, collect};
use crate::parse::ExpressionPart;
use crate::scope::{
    Address, Binder, BodyShape, Callable, CaptureSlot, CaptureSource, Coordinate, Held, Identity,
    Leaf, PathEntry, PathName, Position, Registered, Resolution, Resolved, Root, Segment,
    ShapeError, ShapeKind, Site, Slot, SpelledRoot, Static, StaticCallable, StaticRegistered,
    StaticType, Store, Target, Tree, Unknown, Variable as Located, part_of, path_spelling,
    source_of,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, Interval, KType, Parametric, PathClass, Side, TypeNode, Variable,
    substitute_paths,
};

use super::reads::Part;
use super::trees::Scope;
use super::{Bound, Pass, Tag, under, unknown};

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// An application's pins, by parameter.
type Pins<'p> = BumpVec<'p, (BinderSymbol, Parametric)>;

/// One chain's path entries, each beside its resolution once the walk resolved it and the unit
/// that did, by name: a spelled path's from its first read, a minted one's from where the walk
/// mints it. It opens empty: every walk names a path and a class alike, whatever it read before.
pub(super) struct ChainPaths<'p, 'o, 'graph> {
    root: &'graph BodyShape<'graph>,
    entries: BumpBackedMap<'p, PathName, Entry<'o>>,
    /// What the walk read of each class: the first of its paths it resolved, beside that path's
    /// resolution and the unit that made it.
    classes: BumpBackedMap<'p, PathClass, ClassOf<'o>>,
    /// Each entry being resolved.
    resolving: BumpVec<'p, PathName>,
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

/// A path of a class beside its resolution: what the class reads. Any of the class's paths says
/// alike, since its content names the class; so a read of it gathers nothing.
#[derive(Clone, Copy)]
struct ClassOf<'o> {
    entry: PathEntry<'o>,
    resolution: Resolution<'o>,
}

/// What a path reads of the binder its root names ([`binder_root`](Pass::binder_root)).
struct BinderRoot<'o> {
    tree: Tree<'o>,
    /// The binder's exact signature: the type the binding binds it to, or its closed exact static
    /// type.
    held: Option<KType>,
    /// Its exact static type, where the binding leaves it unbound.
    exact: Option<Parametric>,
    /// A site of the binder's statement.
    site: Site,
}

/// Where a shape reads a class's variable: a path of the class rooted at a binder it reaches,
/// through `chain`, beside where it reads the binder, and what the class reads.
#[derive(Clone, Copy)]
pub(super) struct Witness<'o> {
    binder: Binder,
    chain: &'o [ValueSymbol],
    pub(super) coordinate: Coordinate,
    of: ClassOf<'o>,
}

impl<'o> Witness<'o> {
    /// The content tree the class's paths read their member off.
    pub(super) fn tree(&self) -> Tree<'o> {
        self.of.resolution.tree
    }
}

impl<'o> ChainPaths<'_, 'o, '_> {
    /// The entry `name` as the table holds it.
    fn at_entry(&self, name: PathName) -> Option<Entry<'o>> {
        self.entries.get(&name).copied()
    }

    /// The entry `name`, beside its resolution, where the walk holds it.
    fn at(&self, name: PathName) -> Option<(PathEntry<'o>, Option<(Resolution<'o>, Tag)>)> {
        let held = self.at_entry(name)?;
        Some((held.entry, held.resolution))
    }

    /// Note that `entry` resolved to `resolution`.
    fn resolved(&mut self, entry: PathEntry<'o>, resolution: Resolution<'o>) {
        if let Resolved::Class(class) = resolution.resolved {
            (self.classes)
                .entry(class)
                .or_insert(ClassOf { entry, resolution });
        }
    }
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Open the path table of the chain `root` roots, empty.
    pub(super) fn open_paths(&mut self, root: &'graph BodyShape<'graph>) {
        debug_assert!(
            (self.chain.last())
                .is_some_and(|at| std::ptr::eq(at.shape, root) && at.paths == self.paths.len()),
            "a chain's root opens its table as it is pushed"
        );
        let scratch = self.scratch;
        self.paths.push(ChainPaths {
            root,
            entries: bump_table(scratch),
            classes: bump_table(scratch),
            resolving: BumpVec::new_in(scratch),
        });
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
        table.entries.insert(
            entry.name,
            Entry {
                entry,
                through: true,
                resolution: Some((resolution, tag)),
            },
        );
        table.resolved(entry, resolution);
    }

    /// The resolution of the path `name` of the chain `root` roots, beside the unit that made it,
    /// where the walk has it: its own, or one a standing body's record holds.
    pub(super) fn resolved_at(
        &self,
        root: Segment,
        name: PathName,
    ) -> Option<(Resolution<'o>, Tag)> {
        let table = (self.paths.iter())
            .rev()
            .find(|table| table.root.segment() == root)?;
        match table.at(name) {
            Some((_, Some(held))) => Some(held),
            _ => (self.stood_path(root, name)).map(|(_, resolution, tag)| (resolution, tag)),
        }
    }

    /// The path `name` of the chain `root` roots as the record of a standing body's unit holds it,
    /// at or before the unit birthing the next level, beside its resolution and the tag a read of
    /// it gathers.
    fn stood_path(
        &self,
        root: Segment,
        name: PathName,
    ) -> Option<(PathEntry<'o>, Resolution<'o>, Tag)> {
        self.read_by_content();
        let unit = self.store.holding(Held::Path(root, name))?;
        let level = self
            .level_of(unit.segment)
            .filter(|at| self.standing(*at))?;
        let (tag, facts) = self.stood_record(level, unit.unit)?;
        let (_, (entry, resolution)) = facts.paths.get((root, name))?;
        Some((*entry, *resolution, tag))
    }

    /// What `class`, a class of the chain `root` roots none of whose paths the walk read, reads:
    /// off a record holding one of its paths, one at or before the unit birthing the next level
    /// where that record is a standing body's.
    fn recovered(&self, root: Segment, class: PathClass) -> Option<ClassOf<'o>> {
        self.read_by_content();
        let unit = self.store.holding(Held::Class(root, class))?;
        let (_, facts) = match self.level_of(unit.segment).filter(|at| self.standing(*at)) {
            Some(level) => self.stood_record(level, unit.unit)?,
            None => self.read_unit(unit)?,
        };
        let (_, (entry, resolution)) =
            (facts.paths.iter()).find(|((at, _), (_, resolution))| {
                *at == root && resolution.resolved == Resolved::Class(class)
            })?;
        Some(ClassOf {
            entry: *entry,
            resolution: *resolution,
        })
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
    /// `through`: the one held already, or a new one, named by that content.
    fn path(
        &mut self,
        level: usize,
        root: Root,
        chain: &[ValueSymbol],
        label: TypeSymbol,
        through: Option<Parametric>,
    ) -> PathEntry<'o> {
        let name = PathEntry::minted(root, chain, label, through);
        let table = self.table(level);
        if let Some((entry, _)) = self.paths[table].at(name) {
            return entry;
        }
        let entry = PathEntry {
            root,
            chain: collect(self.lay, chain.iter().copied()),
            label,
            name,
            through,
        };
        self.paths[table].entries.insert(
            name,
            Entry {
                entry,
                through: true,
                resolution: None,
            },
        );
        entry
    }

    /// The entry the shape at `level` spells at `index` of [`BodyShape::spelled`], held from its
    /// first read on, its application read: `None` where the shape numbers it no level.
    fn spelled_entry(&mut self, level: usize, index: usize) -> Option<PathEntry<'o>> {
        let shape = self.chain[level].shape;
        let id = shape.spelled()[index].id?;
        let name = PathName::Spelled(self.shapes.path_level(shape.segment(), index)?);
        let table = self.table(level);
        if self.paths[table].at(name).is_none() {
            let entry = PathEntry {
                root: id.root,
                chain: id.chain,
                label: id.label,
                name,
                through: None,
            };
            self.paths[table].entries.insert(
                name,
                Entry {
                    entry,
                    through: false,
                    resolution: None,
                },
            );
        }
        self.read_through_of(level, name);
        self.paths[table].at(name).map(|(entry, _)| entry)
    }

    /// Read the application of the entry `name` of the chain `level` sits on, where it is a
    /// spelled identity's not read yet: its root's declared type, where it leaves the label an
    /// unpinned head parameter.
    fn read_through_of(&mut self, level: usize, name: PathName) {
        let table = self.table(level);
        let Some(held) = self.paths[table].at_entry(name) else {
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
                self,
                self.types,
                self.scratch,
                declared,
                entry.chain,
                entry.label,
            )
        });
        let slot = (self.paths[table].entries.get_mut(&name)).expect("the entry is held");
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
        let Some(entry) = self.spelled_entry(level, index) else {
            return Ok(());
        };
        let (ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node)) =
            spelled.part
        else {
            unreachable!("the builder records a path's node");
        };
        let spelling = path_spelling(node, self.scratch).map(|spelling| spelling.root);
        // A root written in place is typed where its path resolves, its own types first.
        if let Some(SpelledRoot::Inline(written)) = spelling {
            let holds = |site: Site| match written {
                ExpressionPart::Expression(node) => part_of(shape, node, site).is_some(),
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
        if let Some((_, Some((resolution, tag)))) = self.paths[table].at(entry.name) {
            self.gather(tag);
            return Ok(resolution.resolved);
        }
        let root = self.paths[table].root.segment();
        // A path a standing body's unit resolved stands as that unit resolved it.
        if let Some((held, resolution, tag)) = self.stood_path(root, entry.name) {
            let table = &mut self.paths[table];
            table.entries.insert(
                held.name,
                Entry {
                    entry: held,
                    through: true,
                    resolution: Some((resolution, tag)),
                },
            );
            table.resolved(held, resolution);
            self.gather(tag);
            return Ok(resolution.resolved);
        }
        // A path met while it is being resolved stands for itself.
        if self.paths[table].resolving.contains(&entry.name) {
            return Ok(Resolved::Class(Resolved::own_class(root, entry.name)));
        }
        self.paths[table].resolving.push(entry.name);
        let resolution = self.resolve_entry(level, entry);
        self.paths[table].resolving.pop();
        let (resolution, at) = resolution?;
        if let (Root::Inline { .. }, Resolved::Class(_), Some(at)) =
            (entry.root, resolution.resolved, at)
        {
            self.open_path = Some(at);
        }
        let tag = self.innermost();
        let table = &mut self.paths[table];
        if let Some(held) = table.entries.get_mut(&entry.name) {
            held.resolution = Some((resolution, tag));
        }
        table.resolved(entry, resolution);
        if !self.unfilled {
            self.units[tag.0 as usize]
                .paths
                .push(((root, entry.name), (entry, resolution)));
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
        let (root, held, exact, site, at) = match entry.root {
            Root::Binder(binder) => match self.binder_root(level, binder)? {
                Some(BinderRoot {
                    tree,
                    held,
                    exact,
                    site,
                }) => (tree, held, exact, site, None),
                // A binder off the chain is a leaf the load reads nothing through.
                None => {
                    let leaf = Tree::bound(binder, entry.chain, None);
                    let resolved = Resolved::Class(self.class_of(level, entry, leaf));
                    return Ok((
                        Resolution {
                            tree: leaf,
                            resolved,
                            lacks: None,
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
                        (
                            tree,
                            judge::exactly(self, types, typed),
                            None,
                            site,
                            Some(at),
                        )
                    }
                    None => (Tree::Leaf(Leaf::Unnamed(site)), None, None, site, None),
                }
            }
        };
        let tree =
            (self.read_chain(root, entry.chain, site)?).unwrap_or(Tree::Leaf(Leaf::Unnamed(site)));
        let through = match entry.through {
            None => Some(None),
            Some(through) => judge::closed(self, types, through).map(Some),
        };
        // The signature of the module the root names, or the chain reaches, where the load knows
        // it exactly: the path is then what the run reads off it.
        let view = match tree {
            Tree::View { .. } => (self.view_of(tree, site)?)
                .and_then(|viewed| viewed.built)
                .map(|built| built.view),
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
        let pinned =
            through
                .flatten()
                .and_then(|through| match judge::closed_type_node(types, through) {
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
        // A root exact at an application pinning the label reads that pin, closed or a path's
        // class: at every run the root carries that application, so the path binds what the pin
        // does.
        let read_pin = exact.and_then(|exact| self.read_pin(exact, entry.chain, entry.label));
        let (resolved, lacks) = match (read_pin, pinned.or(closed).or_else(bound)) {
            (Some(resolved), _) => (resolved, None),
            (None, Some(closed)) => (Resolved::Closed(closed), None),
            // A root of an exact signature through a known application that reads no type here
            // reads none at any run.
            (None, None) => (
                Resolved::Class(self.class_of(level, entry, tree)),
                held.filter(|_| through.is_some()),
            ),
        };
        Ok((
            Resolution {
                tree,
                resolved,
                lacks,
            },
            at,
        ))
    }

    /// What a path reads of the binder `binder` its root names, read from `level`. `None` where
    /// the binder's shape is not on the chain.
    fn binder_root(&mut self, level: usize, binder: Binder) -> Read<'o, Option<BinderRoot<'o>>> {
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
        // A module binder the binding binds is exactly the type it carries. One it leaves unbound
        // over open content is what the path waits on, whatever its static type.
        let unknown = self.unknown_at(home, binder.slot);
        let bound = self.binds(unknown);
        if bound.is_none() && !tree.closed() {
            self.note_content(unknown, self.binder_fact(home, binder.slot));
        }
        let (held, exact) = match (bound, self.binder_at(home, binder.slot)) {
            (Some(carried), _) => (Some(carried), None),
            (None, DeclaredType::Type(typed)) => (
                judge::exactly(self, self.types, typed),
                (typed.lower == typed.upper).then_some(typed.upper),
            ),
            (None, DeclaredType::Scheme(_)) => (None, None),
        };
        Ok(Some(BinderRoot {
            tree,
            held,
            exact,
            site,
        }))
    }

    /// The pin of `label` in the application `exact`, a root's exact static type, holds after the
    /// value members `chain`: a closed pin, or a path variable's class. `None` for any other pin,
    /// and where `exact` holds no application pinning `label` there.
    fn read_pin(
        &self,
        exact: Parametric,
        chain: &[ValueSymbol],
        label: TypeSymbol,
    ) -> Option<Resolved> {
        let (types, scratch) = (self.types, self.scratch);
        let held = crate::elaborate::declared_through(self, types, scratch, exact, chain, label)?;
        let TypeNode::SignatureApply { pins, .. } = judge::node_unless_variable(types, held)?
        else {
            return None;
        };
        let pin = pins.get(BinderSymbol::Type(label).symbol())?;
        if let Some(class) = judge::path_class(types, pin) {
            return Some(Resolved::Class(class));
        }
        judge::closed(self, types, pin).map(Resolved::Closed)
    }

    /// `entry`'s class: the paths read at its label, through its application, off a tree of
    /// `tree`'s identity. A tree holding a leaf no binder names is like no other, so its path is
    /// alone in its class.
    fn class_of(&self, level: usize, entry: PathEntry<'o>, tree: Tree<'o>) -> PathClass {
        Resolved::class_of(entry.label, entry.through, tree.identity())
            .unwrap_or_else(|| Resolved::own_class(self.chain_root_segment(level), entry.name))
    }

    /// What each path variable among `variables`, a cell's, reads as in this walk: its entry's
    /// resolution, a closed type or its class's variable now, by the class the cell
    /// named. `None` where one is not resolved yet.
    fn path_points(&self, variables: &[Located]) -> Option<BumpVec<'p, (PathClass, Resolved)>> {
        let mut points = BumpVec::new_in(self.scratch);
        for variable in variables {
            if let Located::Path {
                class, root, entry, ..
            } = *variable
            {
                points.push((class, self.noted(self.resolved_at(root, entry)?).resolved));
            }
        }
        Some(points)
    }

    /// `value`, a type a cell fixed over `variables`, with each path variable at what its entry
    /// resolved to in this walk: a closed path at its type, any other at its class's variable.
    /// `None` where one is not resolved yet.
    pub(super) fn through_paths(
        &self,
        value: Parametric,
        variables: &[Located],
    ) -> Option<Parametric> {
        let points = self.path_points(variables)?;
        Some(self.read_points(value, &points))
    }

    /// `kt` with each path variable `points` holds at what it reads as.
    fn read_points<K: crate::type_lattice::Substitutable>(
        &self,
        kt: K,
        points: &[(PathClass, Resolved)],
    ) -> K {
        let types = self.types;
        substitute_paths(types, self.scratch, kt, &mut |class, label| {
            let (_, resolved) = points.iter().find(|(held, _)| *held == class)?;
            match *resolved {
                Resolved::Closed(closed) => Some(closed.into()),
                Resolved::Class(now) if now == class => None,
                Resolved::Class(now) => Some(types.path_variable(now, label, KType::ANY)),
            }
        })
    }

    /// `declared`, a type or scheme a cell fixed over `variables`, read through its paths'
    /// resolutions, beside whether it is then closed: every variable it names a path that resolved
    /// closed. `None` where a path is not resolved yet, or a scheme reads one the pass cannot
    /// substitute: one of a class, or one beside a name's variable.
    fn declared_through(
        &self,
        declared: DeclaredType<Parametric>,
        variables: &[Located],
    ) -> Option<(DeclaredType<Parametric>, bool)> {
        let points = self.path_points(variables)?;
        let all_closed = variables.len() == points.len()
            && (points.iter()).all(|(_, resolved)| matches!(resolved, Resolved::Closed(_)));
        if matches!(declared, DeclaredType::Scheme(_)) && !all_closed {
            return None;
        }
        Some((self.read_points(declared, &points), all_closed))
    }

    /// A type cell, read through its paths' resolutions: unknown where one is not resolved yet,
    /// closed where every path it reads is.
    pub(super) fn resolved_type(&self, typed: StaticType<'o>) -> StaticType<'o> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        match self.through_paths(value, variables) {
            None => Static::Unknown,
            Some(value) => match judge::static_of(self.types, value) {
                Some(closed) => Static::Closed(closed),
                None => Static::Rigid { value, variables },
            },
        }
    }

    /// A callable cell, read through its paths' resolutions, as
    /// [`resolved_type`](Self::resolved_type) reads a type cell.
    pub(super) fn resolved_callable(&self, typed: StaticCallable<'o>) -> StaticCallable<'o> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        let Some((ktype, closed)) = self.declared_through(value.ktype, variables) else {
            return Static::Unknown;
        };
        let registered = match value.registered {
            None => None,
            Some(registered) => match self.registered_through(registered, variables) {
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
                ktype: judge::static_declared(self.types, value.ktype).expect(CLOSED),
                quantifier_map: value.quantifier_map,
                registered: value
                    .registered
                    .map(|registered| closed_registered(self, registered)),
            }),
            false => Static::Rigid { value, variables },
        }
    }

    /// A registration cell, read through its paths' resolutions.
    pub(super) fn resolved_registered(&self, typed: StaticRegistered<'o>) -> StaticRegistered<'o> {
        let Static::Rigid { value, variables } = typed else {
            return typed;
        };
        if !typed.over_paths() {
            return typed;
        }
        match self.registered_through(value, variables) {
            None => Static::Unknown,
            Some((registered, true)) => Static::Closed(closed_registered(self, registered)),
            Some((value, false)) => Static::Rigid { value, variables },
        }
    }

    fn registered_through(
        &self,
        registered: Registered<'o>,
        variables: &[Located],
    ) -> Option<(Registered<'o>, bool)> {
        let (shape, closed) = self.declared_through(registered.shape, variables)?;
        Some((
            Registered {
                shape,
                ..registered
            },
            closed,
        ))
    }

    /// The static type of the callable body `body`: a point at its function type, or its scheme,
    /// where the load typed it, `[Never, Any]` elsewhere.
    pub(super) fn callable(&self, body: &BodyShape<'graph>) -> Bound {
        let ktype = match self.resolved_callable(self.callable_cell(body)) {
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
        variables.iter().all(|variable| match *variable {
            Located::Type { .. } => true,
            Located::Path { root, entry, .. } => match self.resolved_at(root, entry) {
                Some(held) => match self.noted(held).resolved {
                    Resolved::Class(class) => self.reachable(level, class).is_some(),
                    Resolved::Closed(_) => true,
                },
                // An unresolved path reads as unknown.
                None => true,
            },
        })
    }

    /// Where the shape at `level` reads `binder`, one on its chain: its slot stepped in through
    /// every shape between — a hop per block, and the builder capture reading it per callable or
    /// module. `None` where a shape between captures no such thing, or a quote's code lies between.
    fn binder_coordinate(&self, level: usize, binder: Binder) -> Option<Coordinate> {
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

    /// Where the shape at `level` reads a path of `class`, by the class's content: the leaf
    /// binder its tree reads through, where it is one; else the first binder the shape reaches
    /// that holds a module of the class's tree, itself or at a member chain through module bodies
    /// ([`first_reached`](Self::first_reached)). Each read through an application naming no
    /// path variable. `None` where nothing of the class is known, or no path of it is reachable.
    /// Order-free: whether a class is reachable is what the shape reaches, never which of the
    /// class's paths the walk resolved first. The search is kept per shape and class until
    /// the walk fixes another content tree; a kept answer gathers again what the search gathered.
    pub(super) fn reachable(&self, level: usize, class: PathClass) -> Option<Witness<'o>> {
        // Kept or searched afresh, a read by content holds a witnessing walk's unit.
        self.read_by_content();
        let table = &self.paths[self.table(level)];
        let of = match table.classes.get(&class) {
            Some(of) => *of,
            None => self.recovered(table.root.segment(), class)?,
        };
        let key = (self.chain[level].shape.segment(), class, self.fixings);
        let kept = self.reached.borrow().get(&key).copied();
        let (found, gathered) = match kept {
            Some(kept) => kept,
            None => {
                let mut gathered = BumpVec::new_in(self.scratch);
                let found = self.reach(level, of, &mut gathered);
                let kept = (found, &*gathered.leak());
                self.reached.borrow_mut().insert(key, kept);
                kept
            }
        };
        gathered.iter().for_each(|tag| self.gather(*tag));
        found
    }

    /// [`reachable`](Self::reachable)'s search for the class `of` reads, each unit whose record
    /// it read pushed onto `gathered`.
    fn reach(
        &self,
        level: usize,
        of: ClassOf<'o>,
        gathered: &mut BumpVec<'p, Tag>,
    ) -> Option<Witness<'o>> {
        if !self.locatable(of.entry) {
            return None;
        }
        let witness = |binder: Binder, chain: &'o [ValueSymbol]| {
            let coordinate = self.binder_coordinate(level, binder)?;
            Some(Witness {
                binder,
                chain,
                coordinate,
                of,
            })
        };
        let identity = of.resolution.tree.identity();
        // The class names its path where it holds that path alone, and its leaf binder and chain
        // where its tree is a leaf: whichever of its paths `of` is, the witness is the class's, and
        // locating it reads the chain's text alone, so gathers nothing.
        if Resolved::class_of(of.entry.label, of.entry.through, identity).is_none() {
            let Root::Binder(binder) = of.entry.root else {
                return None;
            };
            return witness(binder, of.entry.chain);
        }
        if let Tree::Leaf(Leaf::Bound { binder, chain, .. }) = of.resolution.tree
            && let Some(found) = witness(binder, chain)
        {
            return Some(found);
        }
        self.first_reached(level, identity, &mut |binder, tree, tag| {
            let mut read = BumpVec::new_in(self.scratch);
            let chain = self.member_chain(tree, identity, &mut read)?;
            let found = witness(binder, chain)?;
            gathered.push(tag);
            gathered.extend(read);
            Some(found)
        })
    }

    /// The first of the module binders the shape at `level` reaches holding a module of
    /// `identity`'s tree, itself or through a member of a module body, that `found` takes, beside
    /// its content tree and the unit that fixed it: those of each shape out to the first callable
    /// or module around `level` — a block reads its encloser's — in slot order, innermost shape
    /// first, then each that callable or module captures, in capture order. Each is found by
    /// `identity`: on a level the walk runs through its own index, on a standing one through the
    /// layers' index, at or before the unit birthing the next level, and read only as it is
    /// offered. So what a shape reaches costs its own text's binders, however large the program
    /// around it.
    fn first_reached<R>(
        &self,
        level: usize,
        identity: Identity,
        found: &mut dyn FnMut(Binder, Tree<'o>, Tag) -> Option<R>,
    ) -> Option<R> {
        let first = self.chain_root(level);
        let mut at = level;
        loop {
            let shape = self.chain[at].shape;
            let body = shape.segment();
            match self.standing(at) {
                false => {
                    for (slot, tag, tree) in self.holding_binders(body, identity) {
                        if let Some(taken) = found(Binder { body, slot }, tree, tag) {
                            return Some(taken);
                        }
                    }
                }
                true => {
                    let mut slots = BumpVec::new_in(self.scratch);
                    self.store
                        .holders(body, identity, &mut |_, slot| slots.push(slot));
                    slots.sort_unstable();
                    slots.dedup();
                    for slot in slots {
                        if let Some((tree, tag)) = self.stood_content(at, slot)
                            && let Some(taken) = found(Binder { body, slot }, tree, tag)
                        {
                            return Some(taken);
                        }
                    }
                }
            }
            match shape.kind() {
                ShapeKind::Block if at > first => at -= 1,
                ShapeKind::Callable | ShapeKind::Module => break,
                _ => return None,
            }
        }
        // Past the first callable or module, a shape reaches what it captures.
        for index in 0..self.chain[at].shape.captures().len() {
            let captured = Coordinate::Activation {
                hops: 0,
                target: Target::Capture(CaptureSlot(index as u32)),
            };
            let super::Landing::Slot { level: home, slot } = self.follow(at, captured).landing
            else {
                continue;
            };
            let body = self.chain[home].shape.segment();
            let held = match self.standing(home) {
                true => self.stood_holding(home, slot, identity),
                false => (self.holding.get(&(body, slot)))
                    .filter(|(_, _, identities)| identities.contains(&identity))
                    .map(|(tree, tag, _)| (*tree, *tag)),
            };
            if let Some((tree, tag)) = held
                && let Some(taken) = found(Binder { body, slot }, tree, tag)
            {
                return Some(taken);
            }
        }
        None
    }

    /// The member chain through which `tree`, a module binder's content tree, holds a module of
    /// `identity`'s tree: empty where it is one, else through the value members of a module body
    /// it is, as the body's own units recorded their content trees, each whose record it read
    /// pushed onto `read`. `None` where it holds none.
    fn member_chain(
        &self,
        tree: Tree<'o>,
        identity: Identity,
        read: &mut BumpVec<'p, Tag>,
    ) -> Option<&'o [ValueSymbol]> {
        let mut chain = BumpVec::new_in(self.scratch);
        let mut bodies = BumpVec::new_in(self.scratch);
        self.member_holding(tree, identity, &mut chain, &mut bodies, read)
            .then(|| collect(self.lay, chain.iter().rev().copied()))
    }

    /// Whether `tree` holds a module of `identity`'s tree, itself or through a member of a module
    /// body it is: its member chain pushed onto `chain` innermost first where it does. A body
    /// already among `bodies` holds nothing more.
    fn member_holding(
        &self,
        tree: Tree<'o>,
        identity: Identity,
        chain: &mut BumpVec<'_, ValueSymbol>,
        bodies: &mut BumpVec<'_, Segment>,
        read: &mut BumpVec<'p, Tag>,
    ) -> bool {
        if tree.identity() == identity {
            return true;
        }
        let Tree::Body { body, .. } = tree else {
            return false;
        };
        if bodies.contains(&body) {
            return false;
        }
        bodies.push(body);
        let shape = self.shape_at(body);
        for slot in (0..shape.slots()).map(|at| Slot(at as u32)) {
            let BinderSymbol::Value(name) = shape.slot_name(slot) else {
                continue;
            };
            let Some((tag, facts)) = self.read_unit(self.shapes.slot_unit(body, slot)) else {
                continue;
            };
            let member = (facts.typed.get(body))
                .and_then(|(_, typed)| typed.contents.get(slot))
                .map(|(_, tree)| *tree);
            if let Some(member) = member
                && self.member_holding(member, identity, chain, bodies, read)
            {
                read.push(tag);
                chain.push(name);
                return true;
            }
        }
        false
    }

    /// The path `witness` reads its class through, held in the table of the chain `level` sits
    /// on: minted where the walk holds none, resolved as the class's paths are by the unit
    /// running.
    pub(super) fn witnessed(&mut self, level: usize, witness: Witness<'o>) -> PathEntry<'o> {
        let label = witness.of.entry.label;
        let through = witness.of.entry.through;
        let entry = self.path(
            level,
            Root::Binder(witness.binder),
            witness.chain,
            label,
            through,
        );
        let table = self.table(level);
        if let Some((_, Some(_))) = self.paths[table].at(entry.name) {
            return entry;
        }
        let resolution = witness.of.resolution;
        if self.unfilled || self.open.is_empty() {
            return entry;
        }
        let tag = self.innermost();
        let root = self.paths[table].root.segment();
        if let Some(held) = self.paths[table].entries.get_mut(&entry.name) {
            held.resolution = Some((resolution, tag));
        }
        self.paths[table].resolved(entry, resolution);
        self.units[tag.0 as usize]
            .paths
            .push(((root, entry.name), (entry, resolution)));
        entry
    }

    /// Whether the application `entry` is read through names no path variable, so a shape can
    /// locate what it names.
    fn locatable(&self, entry: PathEntry<'o>) -> bool {
        let Some(through) = entry.through else {
            return true;
        };
        let mut paths = false;
        judge::read_points(
            self.types,
            self.scratch,
            through,
            Side::Above,
            &mut |variable| {
                paths |= matches!(variable, Variable::Path { .. });
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

    /// `typed`, a static type read in the shape at `level`, with each path variable no path of
    /// whose class that shape reaches read through its bounds: an end at a time, from the side it
    /// bounds.
    pub(super) fn sourced(&self, level: usize, typed: Interval) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let mut unreached = |variable: Variable| {
            let Variable::Path { class, .. } = variable else {
                return None;
            };
            self.reachable(level, class).is_none().then(|| Interval {
                lower: KType::NEVER.into(),
                upper: KType::ANY.into(),
            })
        };
        Interval {
            lower: judge::read_ends(
                self,
                types,
                scratch,
                typed.lower,
                Side::Below,
                &mut unreached,
            ),
            upper: judge::read_ends(
                self,
                types,
                scratch,
                typed.upper,
                Side::Above,
                &mut unreached,
            ),
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
    /// A member is read under these pins, and a value the run retypes by its fit, rooted at a
    /// binder, is typed exactly at their application ([`at_its_paths`](Self::at_its_paths)).
    fn pinned(
        &mut self,
        level: usize,
        root: Root,
        chain: &[ValueSymbol],
        upper: Parametric,
    ) -> Read<'o, Option<(KType, Pins<'p>)>> {
        let types = self.types;
        let (signature, pinned) = match judge::type_node(self, types, upper) {
            TypeNode::Signature { .. } => match judge::closed(self, types, upper) {
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
        // A binder of this shape, its own unit typing it, holds its paths' class before the unit
        // records its content tree for the search to find.
        let typing = chain.is_empty()
            && matches!(root, Root::Binder(binder) if binder.body == self.chain[level].shape.segment());
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
                Resolved::Class(class) if !typing && self.reachable(level, class).is_none() => {
                    return Ok(None);
                }
                Resolved::Class(class) => types.path_variable(class, *name, KType::ANY),
            };
            pins.push((parameter, pin));
            changed = true;
        }
        Ok(changed.then_some((signature, pins)))
    }

    /// Whether [`at_its_paths`](Self::at_its_paths) may pin `typed`: it is at most `declared`, a
    /// signature or an application of one. A variable is declined, not judged: whatever a binding
    /// gives it, at most is right.
    pub(super) fn pins_at_paths(&self, declared: Parametric, typed: Interval) -> bool {
        typed == under(declared)
            && matches!(
                judge::node_unless_variable(self.types, declared),
                Some(TypeNode::Signature { .. } | TypeNode::SignatureApply { .. })
            )
    }

    /// `typed`, the static type of a value the run retypes by its fit to `declared`, rooted at
    /// `root` through `chain` in the shape at `level`: where it is at most `declared`, a signature
    /// or an application leaving a head parameter unpinned, exactly `declared` pinned at the paths
    /// through the value ([`pinned`](Self::pinned)), since the run re-stamps the value at what its
    /// fit solves and each path reads that; `typed` otherwise. A meet stays at most itself. A root
    /// binder the binding binds is exactly the module it carries, so its paths pin nothing.
    pub(super) fn at_its_paths(
        &mut self,
        level: usize,
        root: Root,
        chain: &[ValueSymbol],
        declared: Parametric,
        typed: Interval,
    ) -> Read<'o, Interval> {
        let types = self.types;
        if !self.pins_at_paths(declared, typed) {
            return Ok(typed);
        }
        // Resolving a path is a read by content, which would hold a witnessing walk's unit on
        // every witnessed name for a pin the binding makes moot.
        if let Root::Binder(binder) = root
            && (self.binds(Unknown {
                segment: binder.body,
                at: Address::Slot(binder.slot),
            }))
            .is_some()
        {
            return Ok(typed);
        }
        Ok(match self.pinned(level, root, chain, declared)? {
            Some((signature, pins)) => {
                Interval::point(types.signature_apply(self.scratch, signature, &pins))
            }
            None => typed,
        })
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
        let member = signature_member(self, types, scratch, upper, name);
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
    /// its rule read it. A module member read through a name chain of module members, retyped by
    /// its fit, is pinned at the paths through that chain and the label
    /// ([`at_its_paths`](Self::at_its_paths)); a read through a record's field or a value written
    /// in place stays as it is.
    pub(super) fn read_through_paths(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        label: BinderSymbol,
        typed: Interval,
    ) -> Read<'o, Interval> {
        let BinderSymbol::Value(name) = label else {
            return Ok(typed);
        };
        let read = self.read_off_paths(level, operand, label, typed)?;
        let Some((binder, mut chain)) = self.module_chain(level, operand) else {
            return Ok(read);
        };
        if !self.reads_a_module(level, operand, label) {
            return Ok(read);
        }
        chain.push(name);
        self.at_its_paths(level, Root::Binder(binder), &chain, read.upper, read)
    }

    /// [`name_chain`](Self::name_chain) where every step reads a module's member: the run reads a
    /// path through module members alone, never through a record's field.
    fn module_chain(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<(Binder, BumpVec<'p, ValueSymbol>)> {
        match super::of_part(self.chain[level].shape, part) {
            super::Form::Member {
                operand,
                label: label @ BinderSymbol::Value(name),
                ..
            } => {
                if !self.reads_a_module(level, operand, label) {
                    return None;
                }
                let (binder, mut chain) = self.module_chain(level, operand)?;
                chain.push(name);
                Some((binder, chain))
            }
            _ => self.name_chain(level, part),
        }
    }

    /// Whether reading `label` through `operand`, a part of the shape at `level`, reads a member
    /// of a module: the operand's static upper end is a signature declaring it.
    fn reads_a_module(
        &self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        label: BinderSymbol,
    ) -> bool {
        let Some(operand_type) = self.part_type(level, self.site(level, operand)) else {
            return false;
        };
        signature_member(self, self.types, self.scratch, operand_type.upper, label).is_some()
    }

    /// [`read_through_paths`](Self::read_through_paths)'s read of member `label` through the
    /// operand's own paths.
    fn read_off_paths(
        &mut self,
        level: usize,
        operand: &'graph ExpressionPart<'graph>,
        label: BinderSymbol,
        typed: Interval,
    ) -> Read<'o, Interval> {
        let (types, scratch) = (self.types, self.scratch);
        if judge::is(self, types, typed.upper, KType::NEVER) {
            return Ok(typed);
        }
        let site = self.site(level, operand);
        let Some(operand_type) = self.part_type(level, site) else {
            return Ok(typed);
        };
        let upper = operand_type.upper;
        if !matches!(
            signature_member(self, types, scratch, upper, label),
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
            }) if !matches!(
                judge::type_node(self, types, declared),
                TypeNode::SignatureApply { .. }
            ) || judge::closed(self, types, declared).is_some() =>
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

/// Why a declared type read through paths that all resolved closed is concrete: each path reads a
/// closed type, so no variable is left to reach.
const CLOSED: &str = "a type over closed paths alone is closed";

fn closed_registered<'o, S: Store<'o>>(
    pass: &Pass<'_, 'o, '_, '_, '_, '_, '_, S>,
    registered: Registered<'o>,
) -> Registered<'o, KType> {
    Registered {
        shape: judge::static_declared(pass.types, registered.shape).expect(CLOSED),
        quantifier_map: registered.quantifier_map,
        parameters: registered.parameters,
    }
}

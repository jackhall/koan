//! The load's half of a `:|` view: over an operand whose content tree it sees all of, the static
//! pass lays every type the run's view would intern in the root — the view's own
//! ([`exposure`](view::exposure)) and each nested module's view — so the run interns none of its
//! own.
//!
//! A view's members are its underlying's, so the lay reads what each cell carries, a [`Carried`],
//! off the underlying's tree: a module body's exact signature, which the pass keeps by body; a
//! view's, computed over its source's operand as the run computes it; a header's, which the load
//! computed, each member read through it at the type it declares, as a member read retypes it;
//! and a cell an opaque view rebuilt, as the coercion walk rebuilds it. Each step follows a view
//! to its underlying or a slot into its declared type, so the lay ends at any depth. A view over a
//! closed tree is read once, by its identity, and an open one reads nothing, so a chain of k
//! views, open or closed, costs the load O(k) view reads; every read of a member of such a view or
//! of a rebuilt value is memoized on the pass too ([`ViewIdentity`]).
//!
//! What a `:|` reads of its operand is an [`Operand`] ([`operand_of`](super::super::graph::trees::Apply::operand_of), in
//! tree application): the
//! type it carries, its bare member's digest, and that member as a view — a body a transparent
//! view of itself, a closed view by what the load computed for it, and a module a frame binds by
//! what the frame recorded beside its digest. Where no tree shows a cell — a container literal's,
//! a parameter's, a call's the load does not unfold — its exact type stands for it, which says
//! neither whether the module is a view nor what it is. A nested slot pinning every parameter
//! views any module alike, so the load lays it from types alone
//! ([`pinned_view`](view::pinned_view)); one leaving a parameter unpinned is viewed at run, its
//! carrier keyed on content the load does not see.
//!
//! See [the module design](../../../design/modules.md#the-load-and-the-run).

use crate::elaborate::judge;
use crate::elaborate::signature_member;
use crate::knot::module::coerce::Substitution;
use crate::knot::module::view::{self, Exposure};
use crate::memory::{BumpAllocator, BumpVec, collect, resident};
use crate::scope::{Identity, ShapeError, Site, Store, Tree, hides};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SigSchema, TypeHandle, TypeNode, substitute_carriers,
    substitute_parameters,
};
use crate::values::{Chain, applications};

use super::super::graph::trees::underlying_tree;
use super::Pass;
use super::trees::Scope;

type Read<'o, T> = Result<T, ShapeError<'o>>;

/// What a value carries, as the load knows it.
#[derive(Clone, Copy)]
pub(super) enum Carried<'o> {
    /// A module whose content the load sees: a body, or a view over one.
    Tree(Tree<'o>),
    /// A value known by its exact type alone.
    Typed(KType),
    /// A value a view rebuilt.
    Across(&'o Across<'o>),
}

/// `source` as a view rebuilt it, as the coercion walk does: at `declared`, read across `sides`,
/// the two apart.
pub(super) struct Across<'o> {
    declared: DeclaredType<Parametric>,
    sides: Sided<'o>,
    source: Carried<'o>,
    /// The type the rebuilt value carries, where the load knows it.
    held: Option<KType>,
    /// What a nested module was viewed at: its members are rebuilt across its sides in turn, from
    /// its underlying's, which `source` then carries.
    nested: Option<(SigSchema<'o>, Sided<'o>)>,
}

/// The two sides a view rebuilds a member across — `from*` on its underlying's, `to` on its own —
/// and the carriers the underlying's side reads back, each beside what it hides.
#[derive(Clone, Copy)]
pub(super) struct Sided<'o> {
    from: Substitution<'o>,
    to: Substitution<'o>,
    hidden: &'o [(KType, KType)],
}

/// What a container holds: a list's elements, a dict's values, or a record's field.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Inside {
    Element,
    Value,
    Field(Symbol),
}

/// A value the load reads twice, wherever it is reached from: a view over a closed tree, by its
/// identity, or a value a view rebuilt, by its address. What each yields is memoized on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ViewIdentity {
    Tree(Identity),
    Across(usize),
}

/// One read of a value's content: a module's member, or what a container holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Step {
    Member(ValueSymbol),
    Inside(Inside),
}

/// A value rebuilt by a view: its declared type, the two sides' handles, and the source.
pub(super) type Rebuild = (DeclaredType<Parametric>, KType, KType, ViewIdentity);

/// The modules a value may be: itself, or each cell of a union of cells' signatures.
#[derive(Clone, Copy)]
enum Modules<'p, 'o> {
    One(Carried<'o>),
    Cells(&'p [Carried<'o>]),
}

impl<'o> Modules<'_, 'o> {
    fn as_slice(&self) -> &[Carried<'o>] {
        match self {
            Modules::One(module) => std::slice::from_ref(module),
            Modules::Cells(cells) => cells,
        }
    }
}

/// What `carried` is known by, where a read of it is worth keeping.
fn identity(carried: Carried<'_>) -> Option<ViewIdentity> {
    match carried {
        Carried::Tree(tree) => match tree.identity() {
            identity @ Identity::Closed(_) => Some(ViewIdentity::Tree(identity)),
            _ => None,
        },
        Carried::Across(across) => Some(ViewIdentity::Across(std::ptr::from_ref(across).addr())),
        Carried::Typed(_) => None,
    }
}

/// A declared type read either side of a view.
#[derive(Clone, Copy)]
pub(super) enum Sides {
    /// Both read alike: the coercion walk carries the value as it is.
    Alike,
    /// The two sides, each concrete.
    Apart(KType, KType),
    /// A side still names a variable, and the walk rebuilds nothing.
    Open,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// Lay what the view `viewed` interns over its underlying, which carries `carried`, read at
    /// the `:|` site `site`: each member's declared type read either side, and each nested
    /// module's view.
    pub(super) fn lay(
        &mut self,
        viewed: &Exposure<'o, '_>,
        carried: Carried<'o>,
        site: Site,
    ) -> Read<'o, ()> {
        match self.sided(viewed) {
            Some((sig, sides)) => self.lay_sided(sig, sides, carried, site),
            None => Ok(()),
        }
    }

    /// [`lay`](Self::lay) for a view of schema `sig` rebuilding its members across `sides`.
    fn lay_sided(
        &mut self,
        sig: SigSchema<'o>,
        sides: Sided<'o>,
        carried: Carried<'o>,
        site: Site,
    ) -> Read<'o, ()> {
        self.lay_declared(sig, sides);
        for (name, declared) in sig.value_slots.iter().copied() {
            if let Some(cell) = self.cell(carried, name, site)? {
                self.lay_nested(sides, declared, cell, site)?;
            }
        }
        Ok(())
    }

    /// The schema of the view `viewed` and the sides it rebuilds its members across, its
    /// carriers laid beside them: `None` where the view is open.
    fn sided(&self, viewed: &Exposure<'o, '_>) -> Option<(SigSchema<'o>, Sided<'o>)> {
        let (from, built) = (viewed.from?, viewed.built?);
        let hidden = collect(self.lay, built.record.hidden.iter().copied());
        Some((
            viewed.sig,
            Sided {
                from,
                to: built.to,
                hidden,
            },
        ))
    }

    /// Lay each member's declared type the signature type `carried` gives it, under its pins, as a
    /// member read through a module carrying it reads it: what a module retyped at load interns.
    pub(super) fn lay_carried(&self, carried: KType) {
        let (types, scratch) = (self.types, self.scratch);
        for application in applications(types, scratch, carried) {
            let (signature, pins) = match judge::closed_type_node(types, application) {
                TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
                _ => (application, None),
            };
            let Some(schema) = crate::elaborate::schema_of(signature, types) else {
                continue;
            };
            for (name, _) in schema.value_slots.iter() {
                signature_member(
                    self,
                    types,
                    scratch,
                    carried.into(),
                    BinderSymbol::Value(*name),
                );
            }
            let mut bindings = BumpVec::new_in(scratch);
            bindings.extend(
                pins.iter()
                    .flat_map(|pins| pins.iter())
                    .filter_map(|(name, pin)| match name {
                        BinderSymbol::Type(name) => Some((name, pin)),
                        _ => None,
                    }),
            );
            let bindings = Members::from_pairs(scratch, bindings);
            for declared in schema.keyworded.iter().copied() {
                substitute_parameters(types, scratch, declared, bindings);
            }
        }
    }

    /// Lay each member's declared type of a view of schema `sig` read either side of `sides`, as
    /// the coercion walk reads it before it rebuilds a member.
    fn lay_declared(&self, sig: SigSchema<'o>, sides: Sided<'o>) {
        let declared = sig.value_slots.iter().map(|(_, declared)| *declared);
        for declared in declared.chain(sig.keyworded.iter().copied()) {
            self.read_side(declared, sides, true);
            self.read_side(declared, sides, false);
        }
    }

    /// `declared` read on one of `sides`: under `from*`, each carrier read back, where `from`;
    /// else under `to`.
    fn read_side(
        &self,
        declared: DeclaredType<Parametric>,
        sides: Sided<'o>,
        from: bool,
    ) -> DeclaredType<Parametric> {
        let (types, scratch) = (self.types, self.scratch);
        if !from {
            return substitute_parameters(types, scratch, declared, sides.to.bindings);
        }
        let read = substitute_parameters(types, scratch, declared, sides.from.bindings);
        if sides.hidden.is_empty() {
            return read;
        }
        substitute_carriers(types, scratch, read, &mut |carrier| {
            hides(sides.hidden, carrier)
        })
    }

    /// Lay each nested module's view a member declared `declared`, carrying `cell`, takes under
    /// a view reading `from` on its underlying's side and `to` on its own: in the slot itself, or
    /// in a record's field, a list's element or a dict's value. A nested module is laid wherever
    /// [`nested_view`](Self::nested_view) knows what the run builds; any other is viewed at run.
    fn lay_nested(
        &mut self,
        sides: Sided<'o>,
        declared: DeclaredType<Parametric>,
        cell: Carried<'o>,
        site: Site,
    ) -> Read<'o, ()> {
        let types = self.types;
        let Sides::Apart(src, dst) = self.sides(declared, sides) else {
            return Ok(());
        };
        match declared.view(types).body {
            TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
                let modules = self.modules(cell, site)?;
                for module in modules.as_slice().iter().copied() {
                    if let Some((sig, nested, _)) =
                        self.nested_view(module, src, dst, sides, site)?
                    {
                        let underlying = self.underlying_carried(module);
                        self.lay_sided(sig, nested, underlying, site)?;
                    }
                }
            }
            TypeNode::List { element } => {
                if let Some(cell) = self.inside(cell, Inside::Element, site)? {
                    self.lay_nested(sides, element.into(), cell, site)?;
                }
            }
            TypeNode::Dict { value, .. } => {
                if let Some(cell) = self.inside(cell, Inside::Value, site)? {
                    self.lay_nested(sides, value.into(), cell, site)?;
                }
            }
            TypeNode::Record { fields } => {
                for (binder, field) in fields.iter() {
                    let inside = Inside::Field(binder.symbol());
                    if let Some(cell) = self.inside(cell, inside, site)? {
                        self.lay_nested(sides, field.into(), cell, site)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The view of a nested module carrying `module` at a slot reading `src` on the underlying's
    /// side and `dst` on the view's, under the enclosing view's `sides` — its schema, the sides it
    /// rebuilds its members across and its own signature — where the load knows what the run
    /// builds: over a module whose content tree it sees, closed; over any other, where the slot
    /// pins every parameter, from types alone ([`pinned_view`](view::pinned_view)). A module known
    /// by its type alone, at a slot leaving a parameter unpinned, is viewed at run, its carrier
    /// keyed on content the load does not see.
    fn nested_view(
        &mut self,
        module: Carried<'o>,
        src: KType,
        dst: KType,
        sides: Sided<'o>,
        site: Site,
    ) -> Read<'o, Option<(SigSchema<'o>, Sided<'o>, KType)>> {
        let (types, scratch) = (self.types, self.scratch);
        if let Carried::Tree(tree) = module {
            let operand = self.applying().operand_of(tree, site)?;
            let nested = view::exposure(operand, dst, src, sides.hidden, types, scratch).ok();
            if let Some(nested) = nested
                && let Some(built) = nested.built
                && let Some((sig, sided)) = self.sided(&nested)
            {
                return Ok(Some((sig, sided, built.view)));
            }
        }
        let pinned = view::pinned_view(dst, src, sides.hidden, types, scratch);
        Ok(pinned.map(|pinned| {
            let sides = Sided {
                from: pinned.from,
                to: pinned.to,
                hidden: collect(self.lay, pinned.hidden.iter().copied()),
            };
            (pinned.sig, sides, pinned.view)
        }))
    }

    /// What a view over a module carrying `carried` reads its members from: its underlying's
    /// tree, where the load knows the module's.
    fn underlying_carried(&self, carried: Carried<'o>) -> Carried<'o> {
        match carried {
            Carried::Tree(tree) => Carried::Tree(underlying_tree(self.lay, tree)),
            other => other,
        }
    }

    /// The type a value carrying `carried` carries, where the load knows it exactly.
    pub(super) fn carried_type(
        &mut self,
        carried: Carried<'o>,
        site: Site,
    ) -> Read<'o, Option<KType>> {
        Ok(match carried {
            Carried::Typed(held) => Some(held),
            Carried::Tree(tree) => self.applying().carried(tree, site)?,
            Carried::Across(across) => across.held,
        })
    }

    /// `source` rebuilt by a view at `declared`, read across `sides`: carried as it is where the
    /// two agree.
    fn across(
        &mut self,
        declared: DeclaredType<Parametric>,
        sides: Sided<'o>,
        source: Carried<'o>,
        site: Site,
    ) -> Read<'o, Carried<'o>> {
        let types = self.types;
        let key =
            identity(source).map(|source| (declared, sides.from.handle, sides.to.handle, source));
        if let Some(rebuilt) = key.and_then(|key| self.rebuilds.get(&key)) {
            return Ok(*rebuilt);
        }
        let (held, nested, source) = match self.sides(declared, sides) {
            Sides::Alike => return Ok(source),
            Sides::Open => (None, None, source),
            Sides::Apart(src, dst) => match declared.view(types).body {
                TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
                    // A union of cells' signatures is viewed cell by cell (`modules`), and a
                    // nested view's members are read from its underlying's.
                    let nested = self.nested_view(source, src, dst, sides, site)?;
                    let held = nested.map(|(_, _, view)| view);
                    let nested = nested.map(|(sig, sided, _)| (sig, sided));
                    (held, nested, self.underlying_carried(source))
                }
                // A rebuilt container is laid at its slot's type.
                _ => (Some(dst), None, source),
            },
        };
        let rebuilt = Carried::Across(resident(
            self.lay,
            Across {
                declared,
                sides,
                source,
                held,
                nested,
            },
        ));
        if let Some(key) = key {
            self.rebuilds.insert(key, rebuilt);
        }
        Ok(rebuilt)
    }

    /// What `step` reads of `carried`, memoized where `carried` has an [`ViewIdentity`].
    fn step(
        &mut self,
        carried: Carried<'o>,
        step: Step,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        let key = identity(carried).map(|identity| (identity, step));
        if let Some(read) = key.and_then(|key| self.steps.get(&key)) {
            return Ok(*read);
        }
        let read = match step {
            Step::Member(name) => self.member(carried, name, site)?,
            Step::Inside(inside) => self.held_by(carried, inside, site)?,
        };
        if let Some(key) = key {
            self.steps.insert(key, read);
        }
        Ok(read)
    }

    /// What value member `name` of a module carrying `carried` carries.
    fn cell(
        &mut self,
        carried: Carried<'o>,
        name: ValueSymbol,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        self.step(carried, Step::Member(name), site)
    }

    /// What a container carrying `carried` holds `inside`.
    fn inside(
        &mut self,
        carried: Carried<'o>,
        inside: Inside,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        self.step(carried, Step::Inside(inside), site)
    }

    /// [`cell`](Self::cell), unmemoized.
    fn member(
        &mut self,
        carried: Carried<'o>,
        name: ValueSymbol,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        let (types, scratch) = (self.types, self.scratch);
        Ok(match carried {
            Carried::Typed(held) => slot_type(held, name, types, scratch).map(Carried::Typed),
            Carried::Tree(Tree::Body { body, captures, .. }) => {
                let shape = self.shape_at(body);
                let Some((slot, _)) = shape.slot(BinderSymbol::Value(name)) else {
                    return Ok(None);
                };
                let tree = self.slot_tree(Scope::Body { shape, captures }, slot, site)?;
                // A module the load sees the content of; anything else by the body's type for it.
                let seen = matches!(tree, Tree::Body { .. } | Tree::View { .. })
                    && self.carried_type(Carried::Tree(tree), site)?.is_some();
                if seen {
                    Some(Carried::Tree(tree))
                } else {
                    (self.signature_of(shape))
                        .and_then(|held| slot_type(held, name, types, scratch))
                        .map(Carried::Typed)
                }
            }
            // A view's member is its underlying's, rebuilt.
            Carried::Tree(tree @ Tree::View { underlying, .. }) => {
                let inner = self.cell(Carried::Tree(*underlying), name, site)?;
                let viewed = self.applying().view_of(tree, site)?;
                let viewed = viewed.and_then(|viewed| self.sided(&viewed));
                self.rebuilt(viewed, name, inner, site)?
            }
            // Through a header, the member retyped to the type its carried application declares:
            // a module's tree under a header of its own, anything else by that type where the
            // retype makes it exact.
            Carried::Tree(
                tree @ Tree::Retyped {
                    carried, source, ..
                },
            ) if tree.closed() => {
                // A closed tree's carried type is closed ([`Tree::retyped`]).
                let Some(carried) = judge::static_of(types, carried) else {
                    return Ok(None);
                };
                let Some(declared) = slot_type(carried, name, types, scratch) else {
                    return Ok(None);
                };
                match self.cell(Carried::Tree(*source), name, site)? {
                    Some(Carried::Tree(
                        inner @ (Tree::Body { .. } | Tree::View { .. } | Tree::Retyped { .. }),
                    )) => {
                        let retyped = self.applying().retyped_tree(inner, declared.into(), site)?;
                        Some(Carried::Tree(retyped))
                    }
                    _ => {
                        let retyped = judge::retyped(self, types, scratch, declared.into());
                        judge::exact(self, types, scratch, retyped)
                            .then_some(Carried::Typed(declared))
                    }
                }
            }
            Carried::Tree(_) => None,
            Carried::Across(across) => {
                let inner = self.cell(across.source, name, site)?;
                self.rebuilt(across.nested, name, inner, site)?
            }
        })
    }

    /// Member `name` of a module a view of schema and sides `viewed` rebuilt, its underlying's
    /// member carrying `inner`.
    fn rebuilt(
        &mut self,
        viewed: Option<(SigSchema<'o>, Sided<'o>)>,
        name: ValueSymbol,
        inner: Option<Carried<'o>>,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        let Some((sig, sides)) = viewed else {
            return Ok(None);
        };
        let declared = sig.value_slots.iter().find(|(slot, _)| *slot == name);
        match (declared, inner) {
            (Some((_, declared)), Some(inner)) => {
                Ok(Some(self.across(*declared, sides, inner, site)?))
            }
            _ => Ok(None),
        }
    }

    /// [`inside`](Self::inside), unmemoized.
    fn held_by(
        &mut self,
        carried: Carried<'o>,
        inside: Inside,
        site: Site,
    ) -> Read<'o, Option<Carried<'o>>> {
        let types = self.types;
        Ok(match carried {
            Carried::Typed(held) => {
                held_inside(judge::closed_type_node(types, held), inside).map(Carried::Typed)
            }
            Carried::Tree(_) => None,
            Carried::Across(across) => {
                let declared =
                    held_inside(across.declared.view(types).body, inside).map(DeclaredType::from);
                match (declared, self.inside(across.source, inside, site)?) {
                    (Some(declared), Some(inner)) => {
                        Some(self.across(declared, across.sides, inner, site)?)
                    }
                    _ => None,
                }
            }
        })
    }

    /// The modules a value carrying `carried` may be: a list's cells of several signatures carry
    /// their union, each member one cell's. Only a union's cells are laid, in scratch, and a
    /// rebuilt union's are memoized.
    fn modules(&mut self, carried: Carried<'o>, site: Site) -> Read<'o, Modules<'p, 'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let across = match carried {
            Carried::Typed(held) => {
                return Ok(match judge::closed_type_node(types, held) {
                    TypeNode::Union { members } => {
                        Modules::Cells(collect(scratch, members.iter().map(Carried::Typed)))
                    }
                    _ => Modules::One(carried),
                });
            }
            // A rebuilt module whose type the load knows is one module.
            Carried::Across(across) if across.held.is_none() => across,
            _ => return Ok(Modules::One(carried)),
        };
        let key = ViewIdentity::Across(std::ptr::from_ref(across).addr());
        if let Some(modules) = self.modules.get(&key) {
            return Ok(Modules::Cells(modules));
        }
        let sources = self.modules(across.source, site)?;
        let mut modules = BumpVec::with_capacity_in(sources.as_slice().len(), scratch);
        for module in sources.as_slice().iter().copied() {
            modules.push(self.across(across.declared, across.sides, module, site)?);
        }
        let modules: &'p [Carried<'o>] = modules.leak();
        self.modules.insert(key, modules);
        Ok(Modules::Cells(modules))
    }

    /// `declared` read either side of a view, memoized: a lay reads a member's sides, and then
    /// rebuilds it across them. A view's carriers are a function of its `to`, so the two handles
    /// key the read.
    fn sides(&mut self, declared: DeclaredType<Parametric>, sides: Sided<'o>) -> Sides {
        let key = (declared, sides.from.handle, sides.to.handle);
        let types = self.types;
        let judged = |pass: &Self, src: DeclaredType<Parametric>, dst: DeclaredType<Parametric>| {
            if judge::same(pass, types, pass.scratch, src, dst) {
                return Sides::Alike;
            }
            // A view is laid over a closed tree at a closed application, so its sides hold head
            // parameters and quantifiers, never a run-bound name: reading them closed judges
            // nothing a binding changes.
            match (
                src.as_type().and_then(|src| judge::static_of(types, src)),
                dst.as_type().and_then(|dst| judge::static_of(types, dst)),
            ) {
                (Some(src), Some(dst)) => Sides::Apart(src, dst),
                _ => Sides::Open,
            }
        };
        if let Some((_, src, dst)) = self.sides.get(&key).copied() {
            return judged(self, src, dst);
        }
        let (src, dst) = (
            self.read_side(declared, sides, true),
            self.read_side(declared, sides, false),
        );
        let sides = judged(self, src, dst);
        self.sides.insert(key, (sides, src, dst));
        sides
    }
}

/// What a container type — held, or declared — holds `inside`.
fn held_inside<T: TypeHandle>(node: TypeNode<'_, T>, inside: Inside) -> Option<T> {
    match (node, inside) {
        (TypeNode::List { element }, Inside::Element) => Some(element),
        (TypeNode::Dict { value, .. }, Inside::Value) => Some(value),
        (TypeNode::Record { fields }, Inside::Field(field)) => fields.get(field),
        _ => None,
    }
}

/// The type a module of type `held` gives its value member `name`, where the load knows what the
/// member carries: under its own signature, that signature's; under an application fixing every
/// parameter ([`fixed`]), the signature's declared type read under the pins, itself such an
/// application wherever the member is a nested module.
fn slot_type(
    held: KType,
    name: ValueSymbol,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Option<KType> {
    let read = match module_schema(held, types) {
        Some(schema) => {
            let (_, member) = schema.value_slots.iter().find(|(slot, _)| *slot == name)?;
            *member
        }
        None => {
            let (schema, fixed) = fixed(held, types, scratch)?;
            let (_, declared) = schema.value_slots.iter().find(|(slot, _)| *slot == name)?;
            substitute_parameters(types, scratch, *declared, fixed)
        }
    };
    read.as_type()
        .and_then(|member| judge::static_of(types, member))
}

/// `held`'s schema where it is a module's own signature, declaring no head parameter. A list's
/// element type joined over modules of several signatures, or an empty list's `Never`, is none.
fn module_schema<'run>(held: KType, types: &Chain<'run, '_, '_, '_>) -> Option<SigSchema<'run>> {
    if !matches!(
        judge::closed_type_node(types, held),
        TypeNode::Signature { .. }
    ) {
        return None;
    }
    crate::elaborate::schema_of(held, types).filter(|schema| schema.parameters.is_empty())
}

/// The signature `held` applies and each of its head parameters at its pin, where `held` is an
/// application fixing every one.
fn fixed<'run, 'x>(
    held: KType,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<(SigSchema<'run>, Members<'x, TypeSymbol, KType>)> {
    let TypeNode::SignatureApply { signature, pins } = judge::closed_type_node(types, held) else {
        return None;
    };
    let schema = crate::elaborate::schema_of(signature, types)?;
    let mut fixed = BumpVec::with_capacity_in(schema.parameters.len(), scratch);
    for (parameter, _) in schema.parameters.iter() {
        fixed.push((
            *parameter,
            pins.get(BinderSymbol::Type(*parameter).symbol())?,
        ));
    }
    Some((schema, Members::from_pairs(scratch, fixed)))
}

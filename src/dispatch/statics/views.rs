//! The load's half of a `:|` view: over an operand whose content tree it sees all of, the static
//! pass lays every type the run's view would intern in the root — the view's own
//! ([`opaque_view_types`](view::opaque_view_types)) and each nested module's re-view
//! ([`nested_types`](view::nested_types)) — so the run interns none of its own.
//!
//! The run re-views a nested module from the type the module carries, which the operand's type
//! does not always show. So the lay reads what each cell carries, a [`Carried`], off the operand's
//! tree: a module body's exact signature, which the pass keeps by body; a view's, computed from
//! its source's as the run computes it; a header's, which the load computed, each member read
//! through it at the type it declares, as a member read retypes it; and a cell an opaque view
//! rebuilt, as the coercion walk rebuilds it. Each step follows a view to its source or a slot into its declared type, so the
//! lay ends at any depth. Every read of a view over a closed tree or of a rebuilt value is
//! memoized on the pass ([`Identity`]), so a chain of views costs the load linear time.
//!
//! Where no tree shows a cell — a container literal's, a parameter's, a call's the load does not
//! unfold — its exact type stands for it, and a view that type is is read as its signature declares
//! its members. A member declared at an application fixing every parameter ([`fully_pinned`]) is
//! re-viewed from it as from what it carries, at any depth; one whose application leaves a
//! parameter open is re-viewed at run.
//!
//! See [the module design](../../../design/modules.md#the-load-and-the-run).

use crate::elaborate::signature_member;
use crate::knot::module::coerce::Substitution;
use crate::knot::module::view::{self, ViewTypes};
use crate::memory::{BumpAllocator, BumpVec};
use crate::scope::{Print, ShapeError, Site, Tree};
use crate::symbols::{BinderSymbol, Symbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SigSchema, TypeNode, substitute_parameters,
};
use crate::values::digest::Operator;
use crate::values::{Chain, applications, retyped_to};

use super::Pass;
use super::trees::Scope;

type Read<'graph, T> = Result<T, ShapeError<'graph>>;

/// What a value carries, as the load knows it.
#[derive(Clone, Copy)]
pub(super) enum Carried<'graph, 'p> {
    /// A module whose content the load sees: a body, or a view over one.
    Tree(Tree<'graph>),
    /// A value known by its exact type alone.
    Typed(KType),
    /// A value a view rebuilt.
    Across(&'p Across<'graph, 'p>),
}

/// `source` as a view rebuilt it, as the coercion walk does: at `declared`, read under `from` on
/// the source's side and `to` on the view's, the two apart.
pub(super) struct Across<'graph, 'p> {
    declared: DeclaredType<Parametric>,
    from: Substitution<'graph>,
    to: Substitution<'graph>,
    source: Carried<'graph, 'p>,
    /// The type the rebuilt value carries, where the load knows it.
    held: Option<KType>,
    /// What a nested module was re-viewed at: its members are rebuilt under it in turn.
    nested: Option<ViewTypes<'graph>>,
}

/// What a container holds: a list's elements, a dict's values, or a record's field.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Inside {
    Element,
    Value,
    Field(Symbol),
}

/// A value the load reads twice, wherever it is reached from: a view over a closed tree, by its
/// print, or a value a view rebuilt, by its address. What each yields is memoized on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Identity {
    Tree(Print),
    Across(usize),
}

/// One read of a value's content: a module's member, or what a container holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Step {
    Member(ValueSymbol),
    Inside(Inside),
}

/// A value rebuilt by a view: its declared type, the two sides' handles, and the source.
pub(super) type Rebuild = (DeclaredType<Parametric>, KType, KType, Identity);

/// What `carried` is known by, where a read of it is worth keeping.
fn identity(carried: Carried<'_, '_>) -> Option<Identity> {
    match carried {
        Carried::Tree(tree) => match tree.print() {
            print @ Print::Closed(_) => Some(Identity::Tree(print)),
            _ => None,
        },
        Carried::Across(across) => Some(Identity::Across(std::ptr::from_ref(across).addr())),
        Carried::Typed(_) => None,
    }
}

/// A declared type read either side of a view.
enum Sides {
    /// Both read alike: the coercion walk carries the value as it is.
    Alike,
    /// The two sides, each concrete.
    Apart(KType, KType),
    /// A side still names a variable, and the walk rebuilds nothing.
    Open,
}

impl<'p, 'graph: 'p> Pass<'p, '_, 'graph> {
    /// Lay what the view `viewed` interns over a module that carries `carried`, read at the `:|`
    /// site `site`: each member's declared type read either side, and each nested module's
    /// re-view.
    pub(super) fn lay(
        &mut self,
        viewed: &ViewTypes<'graph>,
        carried: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, ()> {
        self.lay_declared(viewed);
        let Some(from) = viewed.from else {
            return Ok(());
        };
        for (name, declared) in viewed.sig.value_slots.iter().copied() {
            if let Some(cell) = self.cell(carried, name, site)? {
                self.lay_nested(viewed, from, declared, cell, site)?;
            }
        }
        Ok(())
    }

    /// Lay each member's declared type the signature type `carried` gives it, under its pins, as a
    /// member read through a module carrying it reads it: what a module retyped at load interns.
    pub(super) fn lay_carried(&self, carried: KType) {
        let (types, scratch) = (self.types, self.scratch);
        for application in applications(types, scratch, carried) {
            let (signature, pins) = match types.node(application) {
                TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
                _ => (application, None),
            };
            let Some(schema) = crate::elaborate::schema_of(signature, types) else {
                continue;
            };
            for (name, _) in schema.value_slots.iter() {
                signature_member(types, scratch, carried.into(), BinderSymbol::Value(*name));
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

    /// Lay each member's declared type of the view `viewed` read under either side, as the
    /// coercion walk reads it before it rebuilds a member.
    pub(super) fn lay_declared(&self, viewed: &ViewTypes<'graph>) {
        let (types, scratch) = (self.types, self.scratch);
        let declared = viewed.sig.value_slots.iter().map(|(_, declared)| *declared);
        for declared in declared.chain(viewed.sig.keyworded.iter().copied()) {
            for side in viewed.from.iter().chain([&viewed.to]) {
                substitute_parameters(types, scratch, declared, side.bindings);
            }
        }
    }

    /// Lay each nested module's re-view a member declared `declared`, carrying `cell`, takes: in
    /// the slot itself, or in a record's field, a list's element or a dict's value.
    fn lay_nested(
        &mut self,
        viewed: &ViewTypes<'graph>,
        from: Substitution<'graph>,
        declared: DeclaredType<Parametric>,
        cell: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, ()> {
        let (types, scratch) = (self.types, self.scratch);
        let Sides::Apart(src, dst) = sides(types, scratch, declared, from, viewed.to) else {
            return Ok(());
        };
        match declared.view(types).body {
            TypeNode::SignatureApply { .. } => {
                for module in self.modules(cell, site)?.iter().copied() {
                    let Some(held) = self.carried_type(module, site)? else {
                        continue;
                    };
                    if re_viewable(held, types, scratch)
                        && let Ok(nested) = view::nested_types(held, src, dst, types, scratch)
                    {
                        self.lay(&nested, module, site)?;
                    }
                }
            }
            TypeNode::List { element } => {
                if let Some(cell) = self.inside(cell, Inside::Element, site)? {
                    self.lay_nested(viewed, from, element.into(), cell, site)?;
                }
            }
            TypeNode::Dict { value, .. } => {
                if let Some(cell) = self.inside(cell, Inside::Value, site)? {
                    self.lay_nested(viewed, from, value.into(), cell, site)?;
                }
            }
            TypeNode::Record { fields } => {
                for (binder, field) in fields.iter() {
                    let inside = Inside::Field(binder.symbol());
                    if let Some(cell) = self.inside(cell, inside, site)? {
                        self.lay_nested(viewed, from, field.into(), cell, site)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The type a value carrying `carried` carries, where the load knows it exactly.
    pub(super) fn carried_type(
        &mut self,
        carried: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, Option<KType>> {
        Ok(match carried {
            Carried::Typed(held) => Some(held),
            Carried::Tree(Tree::Body { body, .. }) => self.signature_of(self.shape_at(body)),
            Carried::Tree(tree @ Tree::View { .. }) => {
                self.view_of(tree, site)?.map(|viewed| viewed.view)
            }
            // A closed header's type is the one the load computed for it.
            Carried::Tree(tree @ Tree::Retyped { carried, .. }) => tree.closed().then_some(carried),
            Carried::Tree(_) => None,
            Carried::Across(across) => across.held,
        })
    }

    /// What the view `tree` binds, where the load knows it: an opaque view's over its source's
    /// type and its bare member's digest. Memoized by print over a closed tree.
    pub(super) fn view_of(
        &mut self,
        tree: Tree<'graph>,
        site: Site,
    ) -> Read<'graph, Option<ViewTypes<'graph>>> {
        let Tree::View {
            operator,
            application,
            source,
            print,
        } = tree
        else {
            return Ok(None);
        };
        let closed = matches!(print, Print::Closed(_));
        if closed && let Some((viewed, frontier)) = self.views.get(&print).copied() {
            self.note_frontier(frontier);
            return Ok(viewed);
        }
        self.open();
        let viewed = self.view_read(operator, application, source, site);
        let frontier = self.close();
        let viewed = viewed?;
        if closed {
            self.views.insert(print, (viewed, frontier));
        }
        Ok(viewed)
    }

    /// [`view_of`](Self::view_of) unmemoized.
    fn view_read(
        &mut self,
        operator: Operator,
        application: KType,
        source: &'graph Tree<'graph>,
        site: Site,
    ) -> Read<'graph, Option<ViewTypes<'graph>>> {
        let (types, scratch) = (self.types, self.scratch);
        let held = self.carried_type(Carried::Tree(*source), site)?;
        let viewed = match operator {
            Operator::Opaque => source.bare().digest().and_then(|digest| {
                view::opaque_view_types(held, application, digest, types, scratch)
            }),
            Operator::Reviewed => None,
        };
        Ok(viewed)
    }

    /// `source` rebuilt by a view at `declared`, read under `from` and `to`: carried as it is
    /// where the two sides agree.
    fn across(
        &mut self,
        declared: DeclaredType<Parametric>,
        from: Substitution<'graph>,
        to: Substitution<'graph>,
        source: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, Carried<'graph, 'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let key = identity(source).map(|source| (declared, from.handle, to.handle, source));
        if let Some(rebuilt) = key.and_then(|key| self.rebuilds.get(&key)) {
            return Ok(*rebuilt);
        }
        let (held, nested) = match sides(types, scratch, declared, from, to) {
            Sides::Alike => return Ok(source),
            Sides::Open => (None, None),
            Sides::Apart(src, dst) => match declared.view(types).body {
                TypeNode::SignatureApply { .. } => {
                    // A union of cells' signatures is re-viewed cell by cell (`modules`).
                    let held = self.carried_type(source, site)?;
                    let nested = held
                        .filter(|held| re_viewable(*held, types, scratch))
                        .and_then(|held| view::nested_types(held, src, dst, types, scratch).ok());
                    (nested.map(|nested| nested.view), nested)
                }
                // A rebuilt container is laid at its slot's type.
                _ => (Some(dst), None),
            },
        };
        let rebuilt = Carried::Across(scratch.alloc(Across {
            declared,
            from,
            to,
            source,
            held,
            nested,
        }));
        if let Some(key) = key {
            self.rebuilds.insert(key, rebuilt);
        }
        Ok(rebuilt)
    }

    /// What `step` reads of `carried`, memoized where `carried` has an [`Identity`].
    fn step(
        &mut self,
        carried: Carried<'graph, 'p>,
        step: Step,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
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
        carried: Carried<'graph, 'p>,
        name: ValueSymbol,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
        self.step(carried, Step::Member(name), site)
    }

    /// What a container carrying `carried` holds `inside`.
    fn inside(
        &mut self,
        carried: Carried<'graph, 'p>,
        inside: Inside,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
        self.step(carried, Step::Inside(inside), site)
    }

    /// [`cell`](Self::cell), unmemoized.
    fn member(
        &mut self,
        carried: Carried<'graph, 'p>,
        name: ValueSymbol,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
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
            Carried::Tree(tree @ Tree::View { source, .. }) => {
                let inner = self.cell(Carried::Tree(*source), name, site)?;
                let viewed = self.view_of(tree, site)?;
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
                let Some(declared) = slot_type(carried, name, types, scratch) else {
                    return Ok(None);
                };
                match self.cell(Carried::Tree(*source), name, site)? {
                    Some(Carried::Tree(
                        inner @ (Tree::Body { .. } | Tree::View { .. } | Tree::Retyped { .. }),
                    )) => {
                        let level = self.chain.len() - 1;
                        let retyped = self.retyped_tree(level, inner, declared.into(), site)?;
                        Some(Carried::Tree(retyped))
                    }
                    _ => retyped_to(types, declared.into())
                        .is_exact()
                        .then_some(Carried::Typed(declared)),
                }
            }
            Carried::Tree(_) => None,
            Carried::Across(across) => {
                let inner = self.cell(across.source, name, site)?;
                self.rebuilt(across.nested, name, inner, site)?
            }
        })
    }

    /// Member `name` of a module the view `viewed` rebuilt, its source's member carrying `inner`.
    fn rebuilt(
        &mut self,
        viewed: Option<ViewTypes<'graph>>,
        name: ValueSymbol,
        inner: Option<Carried<'graph, 'p>>,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
        let Some(viewed) = viewed else {
            return Ok(None);
        };
        // Over a source the load does not know, the member by the type the view declares for it.
        let (ViewTypes { sig, to, .. }, Some(from)) = (viewed, viewed.from) else {
            return Ok(slot_type(viewed.view, name, self.types, self.scratch).map(Carried::Typed));
        };
        let declared = sig.value_slots.iter().find(|(slot, _)| *slot == name);
        match (declared, inner) {
            (Some((_, declared)), Some(inner)) => {
                Ok(Some(self.across(*declared, from, to, inner, site)?))
            }
            _ => Ok(None),
        }
    }

    /// [`inside`](Self::inside), unmemoized.
    fn held_by(
        &mut self,
        carried: Carried<'graph, 'p>,
        inside: Inside,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
        let types = self.types;
        Ok(match carried {
            Carried::Typed(held) => held_inside(types.node(held), inside).map(Carried::Typed),
            Carried::Tree(_) => None,
            Carried::Across(across) => {
                let declared = declared_inside(across.declared.view(types).body, inside);
                match (declared, self.inside(across.source, inside, site)?) {
                    (Some(declared), Some(inner)) => {
                        Some(self.across(declared, across.from, across.to, inner, site)?)
                    }
                    _ => None,
                }
            }
        })
    }

    /// The modules a value carrying `carried` may be: a list's cells of several signatures carry
    /// their union, each member one cell's. Memoized on a rebuilt union.
    fn modules(
        &mut self,
        carried: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, &'p [Carried<'graph, 'p>]> {
        let (types, scratch) = (self.types, self.scratch);
        let across = match carried {
            Carried::Typed(held) => {
                return Ok(match types.node(held) {
                    TypeNode::Union { members } => {
                        scratch.alloc_slice_fill_iter(members.iter().map(Carried::Typed))
                    }
                    _ => scratch.alloc_slice_copy(&[carried]),
                });
            }
            // A rebuilt module whose type the load knows is one module.
            Carried::Across(across) if across.held.is_none() => across,
            _ => return Ok(scratch.alloc_slice_copy(&[carried])),
        };
        let key = Identity::Across(std::ptr::from_ref(across).addr());
        if let Some(modules) = self.modules.get(&key) {
            return Ok(modules);
        }
        let mut modules = BumpVec::new_in(scratch);
        for module in self.modules(across.source, site)? {
            modules.push(self.across(across.declared, across.from, across.to, *module, site)?);
        }
        let modules: &'p [Carried<'graph, 'p>] = scratch.alloc_slice_copy(&modules);
        self.modules.insert(key, modules);
        Ok(modules)
    }
}

/// `declared` read under `from` on a view's source side and `to` on its own.
fn sides(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    declared: DeclaredType<Parametric>,
    from: Substitution<'_>,
    to: Substitution<'_>,
) -> Sides {
    let read =
        |side: Substitution<'_>| substitute_parameters(types, scratch, declared, side.bindings);
    let (src, dst) = (read(from), read(to));
    if src == dst {
        return Sides::Alike;
    }
    match (
        src.as_type().and_then(|src| types.concrete(src)),
        dst.as_type().and_then(|dst| types.concrete(dst)),
    ) {
        (Some(src), Some(dst)) => Sides::Apart(src, dst),
        _ => Sides::Open,
    }
}

/// What a container type holds `inside`.
fn held_inside(node: TypeNode<'_, KType>, inside: Inside) -> Option<KType> {
    match (node, inside) {
        (TypeNode::List { element }, Inside::Element) => Some(element),
        (TypeNode::Dict { value, .. }, Inside::Value) => Some(value),
        (TypeNode::Record { fields }, Inside::Field(field)) => fields.get(field),
        _ => None,
    }
}

/// What a declared container type declares `inside`.
fn declared_inside(
    node: TypeNode<'_, Parametric>,
    inside: Inside,
) -> Option<DeclaredType<Parametric>> {
    match (node, inside) {
        (TypeNode::List { element }, Inside::Element) => Some(element.into()),
        (TypeNode::Dict { value, .. }, Inside::Value) => Some(value.into()),
        (TypeNode::Record { fields }, Inside::Field(field)) => fields.get(field).map(Into::into),
        _ => None,
    }
}

/// The type a module of type `held` gives its value member `name`, where the load knows what the
/// member carries or re-views alike: under its own signature, that signature's; under an
/// application fixing every parameter ([`fully_pinned`]), the signature's declared type read under
/// the pins, itself such an application wherever the member is a nested module.
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
    read.as_type().and_then(|member| types.concrete(member))
}

/// Whether the load re-views a nested module whose type is `held` as the run does.
fn re_viewable(held: KType, types: &Chain<'_, '_, '_, '_>, scratch: BumpAllocator<'_>) -> bool {
    module_schema(held, types).is_some() || fully_pinned(held, types, scratch)
}

/// `held`'s schema where it is a module's own signature, declaring no head parameter: a type the
/// load knows a nested module's view from, beside a [`fully_pinned`] application. A list's element
/// type joined over modules of several signatures, or an empty list's `Never`, is neither.
fn module_schema<'run>(held: KType, types: &Chain<'run, '_, '_, '_>) -> Option<SigSchema<'run>> {
    if !matches!(types.node(held), TypeNode::Signature { .. }) {
        return None;
    }
    crate::elaborate::schema_of(held, types).filter(|schema| schema.parameters.is_empty())
}

/// Whether `held` is an application fixing every head parameter of its signature: the type a
/// header gives a nested module it declares at one. The run re-views such a module from the
/// application it carries, as the load does, so *fits* solves the re-view to the pins at both.
fn fully_pinned(held: KType, types: &Chain<'_, '_, '_, '_>, scratch: BumpAllocator<'_>) -> bool {
    fixed(held, types, scratch).is_some()
}

/// The signature `held` applies and each of its head parameters at its pin, where `held` is an
/// application fixing every one.
fn fixed<'run, 'x>(
    held: KType,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<(SigSchema<'run>, Members<'x, TypeSymbol, KType>)> {
    let TypeNode::SignatureApply { signature, pins } = types.node(held) else {
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

//! The load's half of a `:|` view: over an operand whose content tree it sees all of, the static
//! pass lays every type the run's view would intern in the root — the view's own
//! ([`view_types`](view::view_types)) and each nested module's re-view
//! ([`nested_types`](view::nested_types)) — so the run interns none of its own.
//!
//! The run re-views a nested module from the type the module carries, which the operand's type
//! does not always show: a transparent view carries each member as it is, yet declares it at its
//! slot's application. So the lay reads what each cell carries, a [`Carried`], off the operand's
//! tree: a module body's exact signature, which the pass keeps by body; a view's, computed from its
//! source's as the run computes it; and a cell an opaque view rebuilt, as the coercion walk rebuilds
//! it. Each step follows a view to its source or a slot into its declared type, so the lay ends
//! at any depth.
//!
//! Where no tree shows a cell — a container literal's, a parameter's, a call's the load does not
//! unfold — its exact type stands for it, and a view that type is is read as its signature declares
//! its members: one level, through an application fixing every parameter ([`fully_pinned`]).
//!
//! See [the module design](../../../design/modules.md#the-load-and-the-run).

use crate::knot::module::ModuleContent;
use crate::knot::module::coerce::Substitution;
use crate::knot::module::view::{self, Ascription, ViewTypes, transparent_view_types};
use crate::memory::{BumpAllocator, BumpVec};
use crate::scope::{ShapeError, Site, Tree};
use crate::symbols::{BinderSymbol, Symbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Parametric, SigSchema, TypeNode, substitute_parameters,
};
use crate::values::Types;
use crate::values::digest::Operator;

use super::trees::Scope;
use super::{Pass, address};

type Read<'graph, T> = Result<T, ShapeError<'graph>>;

/// What a value carries, as the load knows it.
#[derive(Clone, Copy)]
pub(super) enum Carried<'graph, 'p> {
    /// A module whose content the load sees: a body, or a view over one.
    Tree(Tree<'graph>),
    /// A value known by its exact type alone.
    Typed(KType),
    /// `source` as a view rebuilt it: at `declared`, read under `from` on the source's side and
    /// `to` on the view's.
    Across {
        declared: DeclaredType<Parametric>,
        from: Substitution<'graph>,
        to: Substitution<'graph>,
        source: &'p Carried<'graph, 'p>,
    },
}

/// What a container holds: a list's elements, a dict's values, or a record's field.
#[derive(Clone, Copy)]
enum Inside {
    Element,
    Value,
    Field(Symbol),
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
    /// site `site`: each member's declared type read under either side, as the coercion walk reads
    /// it before it rebuilds a member, and each nested module's re-view.
    pub(super) fn lay(
        &mut self,
        viewed: &ViewTypes<'graph>,
        carried: Carried<'graph, 'p>,
        site: Site,
    ) -> Read<'graph, ()> {
        let (types, scratch) = (self.types, self.scratch);
        let declared = viewed.sig.value_slots.iter().map(|(_, declared)| *declared);
        for declared in declared.chain(viewed.sig.keyworded.iter().copied()) {
            for side in viewed.from.iter().chain([&viewed.to]) {
                substitute_parameters(types, scratch, declared, side.bindings);
            }
        }
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
                for module in self.modules(cell) {
                    let Some(held) = self.carried_type(module, site)? else {
                        continue;
                    };
                    if (module_schema(held, types).is_some() || fully_pinned(held, types))
                        && let Ok(nested) = view::nested_types(held, src, dst, types, scratch)
                    {
                        self.lay(&nested, module, site)?;
                    }
                }
            }
            TypeNode::List { element } => {
                if let Some(cell) = self.inside(cell, Inside::Element) {
                    self.lay_nested(viewed, from, element.into(), cell, site)?;
                }
            }
            TypeNode::Dict { value, .. } => {
                if let Some(cell) = self.inside(cell, Inside::Value) {
                    self.lay_nested(viewed, from, value.into(), cell, site)?;
                }
            }
            TypeNode::Record { fields } => {
                for (binder, field) in fields.iter() {
                    let inside = Inside::Field(binder.symbol());
                    if let Some(cell) = self.inside(cell, inside) {
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
        let (types, scratch) = (self.types, self.scratch);
        Ok(match carried {
            Carried::Typed(held) => Some(held),
            Carried::Tree(Tree::Body { shape, .. }) => {
                self.signatures.get(&address(shape)).copied()
            }
            Carried::Tree(Tree::View {
                operator,
                application,
                source,
                ..
            }) => match operator {
                Operator::Transparent => self
                    .carried_type(Carried::Tree(*source), site)?
                    .and_then(|held| transparent_view_types(held, application, types, scratch))
                    .map(|viewed| viewed.view),
                Operator::Opaque => self
                    .opaque(application, source, site)?
                    .map(|viewed| viewed.view),
                Operator::Reviewed => None,
            },
            Carried::Tree(_) => None,
            Carried::Across {
                declared,
                from,
                to,
                source,
            } => {
                let Some(held) = self.carried_type(*source, site)? else {
                    return Ok(None);
                };
                match sides(types, scratch, declared, from, to) {
                    Sides::Alike => Some(held),
                    Sides::Apart(src, dst) => match declared.view(types).body {
                        TypeNode::SignatureApply { .. } => {
                            view::nested_types(held, src, dst, types, scratch)
                                .ok()
                                .map(|nested| nested.view)
                        }
                        // A rebuilt container is laid at its slot's type.
                        _ => Some(dst),
                    },
                    Sides::Open => None,
                }
            }
        })
    }

    /// What the opaque view at `application` of `source` binds, where the load knows the source's
    /// type and digest.
    fn opaque(
        &mut self,
        application: KType,
        source: &'graph Tree<'graph>,
        site: Site,
    ) -> Read<'graph, Option<ViewTypes<'graph>>> {
        let (types, scratch) = (self.types, self.scratch);
        let (Some(held), Some(digest)) = (
            self.carried_type(Carried::Tree(*source), site)?,
            source.digest(),
        ) else {
            return Ok(None);
        };
        let key = ModuleContent::view(Operator::Opaque, application, digest).carrier_key();
        let viewed = view::view_types(
            Some(held),
            application,
            Ascription::Opaque,
            key,
            types,
            scratch,
        );
        Ok(viewed.ok())
    }

    /// What value member `name` of a module carrying `carried` carries.
    fn cell(
        &mut self,
        carried: Carried<'graph, 'p>,
        name: ValueSymbol,
        site: Site,
    ) -> Read<'graph, Option<Carried<'graph, 'p>>> {
        let (types, scratch) = (self.types, self.scratch);
        Ok(match carried {
            Carried::Typed(held) => slot_type(held, name, types).map(Carried::Typed),
            Carried::Tree(Tree::Body {
                shape, captures, ..
            }) => {
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
                    (self.signatures.get(&address(shape)))
                        .and_then(|held| slot_type(*held, name, types))
                        .map(Carried::Typed)
                }
            }
            Carried::Tree(Tree::View {
                operator,
                application,
                source,
                ..
            }) => match operator {
                // A transparent view carries each member as it is.
                Operator::Transparent => self.cell(Carried::Tree(*source), name, site)?,
                Operator::Opaque => {
                    let Some(viewed) = self.opaque(application, source, site)? else {
                        return Ok(None);
                    };
                    let declared = (viewed.sig.value_slots.iter()).find(|(slot, _)| *slot == name);
                    let inner = self.cell(Carried::Tree(*source), name, site)?;
                    match (declared, inner, viewed.from) {
                        (Some((_, declared)), Some(inner), Some(from)) => Some(Carried::Across {
                            declared: *declared,
                            from,
                            to: viewed.to,
                            source: scratch.alloc(inner),
                        }),
                        _ => None,
                    }
                }
                Operator::Reviewed => None,
            },
            Carried::Tree(_) => None,
            Carried::Across {
                declared,
                from,
                to,
                source,
            } => match sides(types, scratch, declared, from, to) {
                Sides::Alike => self.cell(*source, name, site)?,
                Sides::Apart(src, dst) => {
                    let TypeNode::SignatureApply { .. } = declared.view(types).body else {
                        return Ok(None);
                    };
                    let Some(held) = self.carried_type(*source, site)? else {
                        return Ok(None);
                    };
                    let Ok(nested) = view::nested_types(held, src, dst, types, scratch) else {
                        return Ok(None);
                    };
                    let declared = (nested.sig.value_slots.iter()).find(|(slot, _)| *slot == name);
                    let inner = self.cell(*source, name, site)?;
                    match (declared, inner, nested.from) {
                        (Some((_, declared)), Some(inner), Some(from)) => Some(Carried::Across {
                            declared: *declared,
                            from,
                            to: nested.to,
                            source: scratch.alloc(inner),
                        }),
                        _ => None,
                    }
                }
                Sides::Open => None,
            },
        })
    }

    /// What a container carrying `carried` holds `inside`.
    fn inside(&self, carried: Carried<'graph, 'p>, inside: Inside) -> Option<Carried<'graph, 'p>> {
        let (types, scratch) = (self.types, self.scratch);
        match carried {
            Carried::Typed(held) => held_inside(types.node(held), inside).map(Carried::Typed),
            Carried::Tree(_) => None,
            Carried::Across {
                declared,
                from,
                to,
                source,
            } => {
                let inner = self.inside(*source, inside);
                match sides(types, scratch, declared, from, to) {
                    Sides::Alike => inner,
                    Sides::Apart(..) => {
                        let declared = declared_inside(declared.view(types).body, inside);
                        match (declared, inner) {
                            (Some(declared), Some(inner)) => Some(Carried::Across {
                                declared,
                                from,
                                to,
                                source: scratch.alloc(inner),
                            }),
                            _ => None,
                        }
                    }
                    Sides::Open => None,
                }
            }
        }
    }

    /// The modules a value carrying `carried` may be: a list's cells of several signatures carry
    /// their union, each member one cell's.
    fn modules(&self, carried: Carried<'graph, 'p>) -> BumpVec<'p, Carried<'graph, 'p>> {
        let (types, scratch) = (self.types, self.scratch);
        let mut modules = BumpVec::new_in(scratch);
        match carried {
            Carried::Typed(held) => match types.node(held) {
                TypeNode::Union { members } => {
                    modules.extend(members.iter().map(Carried::Typed));
                }
                _ => modules.push(carried),
            },
            Carried::Tree(_) => modules.push(carried),
            Carried::Across {
                declared,
                from,
                to,
                source,
            } => {
                for module in self.modules(*source) {
                    modules.push(Carried::Across {
                        declared,
                        from,
                        to,
                        source: scratch.alloc(module),
                    });
                }
            }
        }
        modules
    }
}

/// `declared` read under `from` on a view's source side and `to` on its own.
fn sides(
    types: &Types<'_, '_>,
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

/// The type a module whose own signature is `held` gives its value member `name`.
fn slot_type(held: KType, name: ValueSymbol, types: &Types<'_, '_>) -> Option<KType> {
    let schema = module_schema(held, types)?;
    let (_, member) = schema.value_slots.iter().find(|(slot, _)| *slot == name)?;
    member.as_type().and_then(|member| types.concrete(member))
}

/// `held`'s schema where it is a module's own signature, declaring no head parameter: a type the
/// load knows a nested module's view from, beside a [`fully_pinned`] application. A list's element
/// type joined over modules of several signatures, or an empty list's `Never`, is neither.
fn module_schema<'run>(held: KType, types: &Types<'_, 'run>) -> Option<SigSchema<'run>> {
    if !matches!(types.node(held), TypeNode::Signature { .. }) {
        return None;
    }
    crate::elaborate::schema_of(held, types).filter(|schema| schema.parameters.is_empty())
}

/// Whether `held` is an application fixing every head parameter of its signature: the type a
/// transparent view's signature gives a nested module. *Fits* solves a re-view from it to its pins,
/// the bindings a re-view from the module's own signature solves at the same slot, so the load
/// lays the view the run builds.
fn fully_pinned(held: KType, types: &Types<'_, '_>) -> bool {
    let TypeNode::SignatureApply { signature, pins } = types.node(held) else {
        return false;
    };
    crate::elaborate::schema_of(signature, types).is_some_and(|schema| {
        schema
            .parameters
            .iter()
            .all(|(name, _)| pins.get(BinderSymbol::Type(*name).symbol()).is_some())
    })
}

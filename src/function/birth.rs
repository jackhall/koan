//! The tie: one component of value binders born together as one knot.
//!
//! A member is a callable binder, whose node is a function; a `MODULE` or `GROUP` binder, whose
//! node is a module; or a `LET` of a value name whose right-hand side is rooted at a constructor,
//! whose node is data (see [`data`](super::data)); anything else refuses the tie before a member is
//! read.
//!
//! Everything a member needs is then read into scratch with no writer in reach — a function's
//! body, its type elaborated from its signature and its captures; a data member's cells, with a
//! part only the caller can evaluate asked of its evaluator by site —
//! every mention of a fellow member minted as an edge into the knot about to be tied. Container
//! memos are derived and every construction checked, and a read still pending, a part the caller
//! has not evaluated, a cycle of containers or a construction that misfits refuses the tie before a
//! byte is written. Only then are the closure runs and data nodes laid down, the knot's weight
//! summed, and the nodes tied: member `i` is node `i`, and the anonymous data nodes follow.
//!
//! A module member is born body-first and alone: its binder's body has already run in an activation
//! the caller supplies through [`Supplied::Body`], and the tie reads that activation's slots out in
//! slot order — which is layout order — and asks `elaborate` for the self-signature over them. So a
//! module's type and weight are facts about the members its body bound, and the node is written
//! once.

use crate::elaborate::{Elaboration, Unsigned, callable_type, self_signature};
use crate::memory::{BumpAllocator, BumpVec, CellHandle, Knot, KnotPlan, Writer};
use crate::parse::ExpressionPart;
use crate::scope::{
    Binding, BodyShape, ClosureBindings, ClosureRefused, Component, ShapeKind, Site,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{ConstructionRefused, KeyRejected, Link, Weight};

use super::data::{self, Stager};
use super::module::module;
use super::{Function, KActivation, KValue, Knotted, Node};

/// What the caller supplies for a part only it can produce: an evaluated value for a data member's
/// part, or a module binder's body already run.
#[derive(Clone, Copy)]
pub enum Supplied<'graph, 'cell> {
    Value(KValue<'graph, 'cell>),
    /// A module binder's body, run: its activation with every slot bound.
    Body(&'cell KActivation<'graph, 'cell>),
}

/// Why a component could not be tied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untieable<'x> {
    /// A read — a capture, a signature's type name, a data member's mention, a construction's head
    /// — whose binder is still running.
    Pending {
        name: BinderSymbol,
        binder: CellHandle,
    },
    /// A member's signature did not elaborate.
    Type(Elaboration),
    /// A member that is neither a callable binder, nor a module binder, nor a `LET` of a value name
    /// whose right-hand side, through one-part groups, is a list, dict or record literal or a
    /// nominal construction.
    Opaque { name: BinderSymbol },
    /// Data member `name`'s right-hand side has a part at `site` the caller must evaluate first.
    Eager { name: BinderSymbol, site: Site },
    /// A dict key in data member `name` at `site` evaluated to something no key can be.
    Key {
        name: BinderSymbol,
        site: Site,
        rejected: KeyRejected,
    },
    /// A construction at `site` in data member `name` the construction rule refuses.
    Construction {
        name: BinderSymbol,
        site: Site,
        refused: ConstructionRefused,
    },
    /// Container nodes that reach one another with no function or tagged node between them, so no
    /// finite type memoizes them: the members whose right-hand sides hold them, in component order.
    TypeCycle { names: &'x [BinderSymbol] },
}

/// A function member, read and not yet written.
struct StagedFunction<'graph, 'cell, 'x> {
    shape: &'graph BodyShape<'graph>,
    ktype: KType,
    captures: BumpVec<'x, Link<'graph, 'cell, Knotted<'graph, 'cell>>>,
}

/// Tie `component` of `activation`'s shape as one knot in `writer`'s region: every member born
/// together, each closure binding and data cell a value word or an edge into this knot, each part
/// only the caller can evaluate read from `eager` by site. A refusal writes nothing.
///
/// Member `i` of `component` is the knot's node `i`; the caller binds each member's slot to the
/// member at that node.
pub fn tie<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    eager: &mut dyn FnMut(Site) -> Option<Supplied<'graph, 'cell>>,
) -> Result<Knot<'cell, Node<'graph, 'cell>>, Untieable<'x>> {
    debug_assert!(
        component.deferred_only,
        "the shape builder refuses a component with an eager cycle"
    );
    let shape = activation.shape();
    let names = |owner: u32| shape.slot_name(component.members[owner as usize]);
    let mut roots: BumpVec<'x, Option<&'graph ExpressionPart<'graph>>> =
        BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        match shape.births(*slot).map(BodyShape::kind) {
            // A module is alone in its component and shares nothing with the staging below.
            Some(ShapeKind::Module) => {
                return module_member(writer, activation, component, types, scratch, eager);
            }
            Some(_) => {
                roots.push(None);
                continue;
            }
            None => {}
        }
        let value_binder = matches!(shape.slot_name(*slot), BinderSymbol::Value(_));

        match shape
            .rhs(*slot)
            .filter(|_| value_binder)
            .and_then(|part| data::root(shape, part))
        {
            Some(root) => roots.push(Some(root)),
            None => {
                return Err(Untieable::Opaque {
                    name: shape.slot_name(*slot),
                });
            }
        }
    }
    let nodes = Stager::nodes(activation, component, &roots, scratch, eager)?;
    let plan = KnotPlan::new(nodes.len() as u32);
    let functions = stage_functions(&plan, activation, component, types, scratch)?;

    let mut memos: BumpVec<'x, Option<KType>> = BumpVec::with_capacity_in(nodes.len(), scratch);
    memos.extend((0..nodes.len()).map(|index| {
        functions
            .get(index)
            .and_then(|function| function.as_ref())
            .map(|function| function.ktype)
    }));
    data::memos(&nodes, &mut memos, &names, types, scratch)?;
    data::check(&nodes, &memos, &names, types, scratch)?;

    let mut knot_weight = Weight::flat::<usize>();
    let mut closures = BumpVec::with_capacity_in(functions.len(), scratch);
    for function in functions.iter() {
        closures.push(function.as_ref().map(|function| {
            let closure = ClosureBindings::of(writer, &function.captures);
            knot_weight = knot_weight.plus(closure.weight());
            closure
        }));
    }
    let mut circulars = BumpVec::with_capacity_in(nodes.len(), scratch);
    for (index, node) in nodes.iter().enumerate() {
        circulars.push(node.as_ref().map(|node| {
            let memo = memos[index].expect("every node's memo is derived");
            let circular = data::lay_down(writer, node, memo, &plan, types, scratch);
            knot_weight = knot_weight.plus(circular.weight());
            circular
        }));
    }
    knot_weight = (0..nodes.len()).fold(knot_weight, |weight, _| {
        weight.plus(Weight::flat::<Node<'graph, 'cell>>())
    });
    Ok(plan.tie(writer, |edge| {
        let index = edge.index() as usize;
        match (
            functions.get(index).and_then(Option::as_ref),
            circulars[index],
        ) {
            (Some(function), _) => Node::Function(Function {
                ktype: function.ktype,
                shape: function.shape,
                closure: closures[index].expect("a function member has a closure"),
                knot_weight,
            }),
            (None, Some(circular)) => Node::Data {
                circular,
                knot_weight,
            },
            (None, None) => unreachable!("every node is a function or a data node"),
        }
    }))
}

/// Tie the lone module member of `component`: the caller's supplied body, read out in slot order.
fn module_member<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    eager: &mut dyn FnMut(Site) -> Option<Supplied<'graph, 'cell>>,
) -> Result<Knot<'cell, Node<'graph, 'cell>>, Untieable<'x>> {
    let shape = activation.shape();
    let [slot] = component.members else {
        unreachable!("a module binder is alone in its component: the shape refuses a cycle")
    };
    debug_assert!(!component.cyclic);
    let name = shape.slot_name(*slot);
    let site = shape
        .birth_site(*slot)
        .expect("a module binder's body sits at a site of its own");
    let body = match eager(site) {
        Some(Supplied::Body(body)) => body,
        Some(Supplied::Value(_)) => unreachable!("a module member is asked for its run body"),
        None => return Err(Untieable::Eager { name, site }),
    };
    debug_assert!(std::ptr::eq(
        body.shape(),
        shape.births(*slot).expect("this binder births its body"),
    ));
    let ktype = self_signature(body, types, scratch)
        .map_err(|Unsigned { name, binder }| Untieable::Pending { name, binder })?;
    let mut members = BumpVec::with_capacity_in(body.shape().slots(), scratch);
    for (_, binding) in body.slots() {
        match binding {
            Binding::Bound(value) => members.push(value),
            Binding::Pending(binder) => {
                unreachable!("the self-signature refused a claimed slot first: {binder:?}")
            }
        }
    }
    Ok(module(writer, ktype, &members))
}

/// Read every function member of `component` into `scratch`, by member index; `None` for a data
/// member.
fn stage_functions<'graph, 'cell, 'x>(
    plan: &KnotPlan,
    activation: &KActivation<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, Option<StagedFunction<'graph, 'cell, 'x>>>, Untieable<'x>> {
    let shape = activation.shape();
    let mut staged = BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        let Some(body) = shape.births(*slot) else {
            staged.push(None);
            continue;
        };
        let form = body.form().expect("a callable body sits in its form");
        let ktype =
            callable_type(form, activation, types, scratch).map_err(|error| match error {
                Elaboration::Pending { name, binder } => Untieable::Pending {
                    name: BinderSymbol::Type(name),
                    binder,
                },
                error => Untieable::Type(error),
            })?;
        let captures = ClosureBindings::read_captures(body, activation, scratch, |index| {
            plan.edge(index)
                .expect("a member index is below the knot's count")
        })
        .map_err(|ClosureRefused { name, pending }| Untieable::Pending {
            name,
            binder: pending,
        })?;
        staged.push(Some(StagedFunction {
            shape: body,
            ktype,
            captures,
        }));
    }
    Ok(staged)
}

impl<'graph, 'cell> Knotted<'graph, 'cell> {
    /// The member at node `index` of a knot [`tie`] laid down — member `index` of its component,
    /// or an anonymous data node past them.
    pub fn of(knot: Knot<'cell, Node<'graph, 'cell>>, index: usize) -> Self {
        Knotted(
            knot.members()
                .nth(index)
                .expect("a member index is below the knot's count"),
        )
    }
}

//! The tie: one component of callable binders born together as one knot.
//!
//! Everything a member needs is read first, into scratch, with no writer in reach — its body, its
//! type elaborated from its signature, and its captures, a capture of a fellow member minted as an
//! edge into the knot about to be tied. A member that is not a callable binder refuses the tie before
//! any member is read, and a read still pending refuses it before a byte is written. Only then are
//! the closure runs laid down, the knot's weight summed, and the nodes tied in member order, so
//! member `i` is the node edge `i` names.

use crate::elaborate::{Elaboration, callable_type};
use crate::memory::{BumpAllocator, BumpVec, CellHandle, Knot, KnotPlan, Writer};
use crate::parse::BinderSymbol;
use crate::scope::{Capture, ClosureBindings, ClosureRefused, Component, Shape};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::Weight;

use super::{Callable, Function, KActivation, Node};

/// Why a component could not be tied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untieable {
    /// A member whose binder births no callable — a data binder; a component holding one is a
    /// circular value, not a knot of functions.
    Data { name: BinderSymbol },
    /// A capture or a signature's type name whose binder is still running.
    Pending {
        name: BinderSymbol,
        binder: CellHandle,
    },
    /// A member's signature did not elaborate.
    Type(Elaboration),
}

/// One member, read and not yet written.
struct Staged<'graph, 'cell, 'x> {
    shape: &'graph Shape<'graph>,
    ktype: KType,
    captures: BumpVec<'x, Capture<'graph, 'cell, Callable<'graph, 'cell>>>,
}

/// Tie `component` of `activation`'s shape as one knot in `writer`'s region: every member born
/// together, each closure binding a value word or an edge into this knot. A refusal writes nothing.
///
/// Member `i` of `component` is the knot's node `i`; the caller binds each member's slot to the
/// callable at that node.
pub fn tie<'graph, 'cell>(
    writer: Writer<'cell>,
    activation: &KActivation<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<Knot<'cell, Node<'graph, 'cell>>, Untieable> {
    debug_assert!(
        component.deferred_only,
        "the shape builder refuses a component with an eager cycle"
    );
    let plan = KnotPlan::new(component.members.len() as u32);
    let staged = stage(&plan, activation, component, types, scratch)?;
    let mut closures = BumpVec::with_capacity_in(staged.len(), scratch);
    let mut knot_weight = Weight::flat::<usize>();
    for member in staged.iter() {
        let closure = ClosureBindings::of(writer, &member.captures);
        knot_weight = knot_weight
            .plus(Weight::flat::<Node<'graph, 'cell>>())
            .plus(closure.weight());
        closures.push(closure);
    }
    Ok(plan.tie(writer, |edge| {
        let member = &staged[edge.index() as usize];
        Node::Function(Function {
            ktype: member.ktype,
            shape: member.shape,
            closure: closures[edge.index() as usize],
            knot_weight,
        })
    }))
}

/// Read every member of `component` into `scratch`.
fn stage<'graph, 'cell, 'x>(
    plan: &KnotPlan,
    activation: &KActivation<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, Staged<'graph, 'cell, 'x>>, Untieable> {
    let shape = activation.shape();
    if let Some(slot) = component
        .members
        .iter()
        .find(|slot| shape.births(**slot).is_none())
    {
        return Err(Untieable::Data {
            name: shape.slot_name(*slot),
        });
    }
    let mut staged = BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        let body = shape.births(*slot).expect("every member births a callable");
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
                .expect("a member index is below the component's count")
        })
        .map_err(|ClosureRefused { name, pending }| Untieable::Pending {
            name,
            binder: pending,
        })?;
        staged.push(Staged {
            shape: body,
            ktype,
            captures,
        });
    }
    Ok(staged)
}

impl<'graph, 'cell> Callable<'graph, 'cell> {
    /// The callable at node `index` of a knot [`tie`] laid down — member `index` of its component.
    pub fn of(knot: Knot<'cell, Node<'graph, 'cell>>, index: usize) -> Self {
        Callable(
            knot.members()
                .nth(index)
                .expect("a member index is below the knot's count"),
        )
    }
}

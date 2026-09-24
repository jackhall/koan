//! The tie: one component of value binders born together as one knot.
//!
//! A member is a callable binder, whose node is a [function](super::function); a `MODULE` or
//! `GROUP` binder, whose node is a [module](super::module) — alone in its component, so it ties on
//! its own path; or a `LET` of a value name whose right-hand side is rooted at a constructor, whose
//! node is [data](super::data); anything else refuses the tie before a member is read.
//!
//! Everything a member needs is then read into scratch with no writer in reach — a function's
//! body, its type elaborated from its signature and its captures; a data member's cells, with a
//! part only the caller can evaluate asked of its evaluator by site —
//! every mention of a fellow member minted as an edge into the knot about to be tied. Container
//! memos are derived and every construction checked, and a part the caller has not evaluated, a
//! cycle of containers or a construction that misfits refuses the tie before a byte is written.
//! Only then are the closure runs and data nodes laid down, the knot's weight summed, and the nodes
//! tied: member `i` is node `i`, and the anonymous nodes follow. A `FN` a data member holds that
//! captures a fellow member is one of them, a function node staged like a function member.

use crate::memory::{BumpAllocator, BumpVec, Knot, KnotPlan, Writer};
use crate::parse::ExpressionPart;
use crate::scope::{BodyShape, Component, ShapeKind};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::Weight;

use super::data::{self, Stager};
use super::{Eager, Function, KActivationView, Knotted, Node, Untieable, function, module};

/// Tie `component` of `activation`'s shape as one knot in `writer`'s region: every member born
/// together, each closure binding and data cell a value word or an edge into this knot, each part
/// only the caller can evaluate read from `eager` by site. `eager` is asked for every such part in
/// one attempt, handed the part itself — or `None` for a module body — so a caller refused on
/// several learns them all at once. A refusal writes nothing.
///
/// Member `i` of `component` is the knot's node `i`; the caller binds each member's slot to the
/// member at that node.
pub fn tie<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    activation: &KActivationView<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    eager: &mut Eager<'_, 'graph, 'cell>,
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
                return module::tie_member(writer, activation, component, types, scratch, eager);
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
    let nodes = Stager::nodes(activation, component, &roots, types, scratch, eager)?;
    let plan = KnotPlan::new(nodes.len() as u32);
    let mut bodies = BumpVec::with_capacity_in(nodes.len(), scratch);
    bodies.extend(nodes.iter().enumerate().map(|(index, node)| match node {
        None => shape.births(component.members[index]),
        Some(node) => node.function(),
    }));
    let functions = function::stage(&plan, activation, &bodies, types, scratch)?;

    let mut memos: BumpVec<'x, Option<KType>> = BumpVec::with_capacity_in(nodes.len(), scratch);
    memos.extend(
        functions
            .iter()
            .map(|staged| staged.as_ref().map(|staged| staged.ktype)),
    );
    data::memos(&nodes, &mut memos, &names, types, scratch)?;
    data::check(&nodes, &memos, &names, types, scratch)?;

    let mut knot_weight = Weight::flat::<usize>();
    // Each function node's typing record (its quantifier map and registered shape) is laid down
    // once, beside its closure: it lives in the region for the knot's life, and a copy re-homes it
    // through the destination writer.
    let mut laid = BumpVec::with_capacity_in(functions.len(), scratch);
    for staged in functions.iter() {
        laid.push(staged.as_ref().map(|staged| {
            let (closure, typing, weight) = staged.laid_down(writer);
            knot_weight = knot_weight.plus(weight);
            (closure, typing)
        }));
    }
    let mut circulars = BumpVec::with_capacity_in(nodes.len(), scratch);
    for (index, node) in nodes.iter().enumerate() {
        let data = node.as_ref().filter(|node| node.function().is_none());
        circulars.push(data.map(|node| {
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
        match (functions[index].as_ref().zip(laid[index]), circulars[index]) {
            (Some((staged, (closure, typing))), _) => Node::Function(Function::new(
                staged.ktype,
                typing,
                staged.shape,
                closure,
                knot_weight,
            )),
            (None, Some(circular)) => Node::Data {
                circular,
                knot_weight,
            },
            (None, None) => unreachable!("every node is a function or a data node"),
        }
    }))
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

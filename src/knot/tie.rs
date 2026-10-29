//! The tie: one component of value binders born together as one knot.
//!
//! A member is a callable binder, whose node is a [function](super::function); a `MODULE` or
//! `GROUP` binder, whose node is a [module](super::module) — alone in its component, so it ties on
//! its own path; or a `LET` of a value name whose right-hand side is rooted at a constructor, whose
//! node is [data](super::data), or at a quote reading a fellow member, whose node is
//! [code](super::code); anything else refuses the tie before a member is read.
//!
//! Everything a member needs is then read into scratch with no writer in reach — a function's
//! body, its type elaborated from its signature and its captures; a code node's `$` bindings; a
//! data member's cells, with a
//! part only the caller can evaluate asked of its evaluator by site —
//! every mention of a fellow member minted as an edge into the knot about to be tied. Each member
//! is born at its annotation: an annotated function member takes the annotation's retype as its
//! type and keeps the signature its body was checked against, an annotated code member takes it as
//! its carried type, and an annotated data member's memo is its derived memo retyped as soon as it
//! is derived — so a fellow's edge sees what the
//! member's slot holds. Every construction is then checked, and a part the caller has not
//! evaluated, a cycle of derived nodes or a construction the rule refuses stops the tie before a
//! byte is written.
//! Only then are the closure runs, bound runs and data nodes laid down — each list node a node
//! spreads before it — the knot's weight summed,
//! and the nodes tied: member `i` is node `i`, and the anonymous nodes follow. A `FN` a data member
//! holds that captures a fellow member is one of them, a function node staged like a function
//! member, and so is a quote that reads one. The tie hands back each annotation it resolved beside
//! the knot ([`Tied`]), so the caller holds each slot to it without resolving it again.

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpVec, Knot, KnotPlan, Writer};
use crate::parse::ExpressionPart;
use crate::scope::{BodyShape, Component, ShapeKind, Stamp, Written};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, satisfied_by};
use crate::values::{ArenaRef, Chain, Retype, Weight, retype_target_as};

use super::data::{self, Stager};
use super::{Eager, Function, KReading, Knotted, Node, Untieable, code, function, module};

/// A knot [`tie`] laid down, beside each member's annotation as the tie resolved it, by member
/// index: `None` where the member states none, or it does not resolve. The caller holds each slot
/// to it without resolving it again.
pub struct Tied<'graph, 'cell, 'x> {
    pub knot: Knot<'cell, Node<'graph, 'cell>>,
    pub annotations: BumpVec<'x, Option<KType>>,
}

/// Tie `component` of `activation`'s shape as one knot in `writer`'s region: every member born
/// together, each closure binding and data cell a value word or an edge into this knot, each part
/// only the caller can evaluate read from `eager` by site. `eager` is asked for every such part in
/// one attempt, handed the part itself — or `None` for a module body — so a caller refused on
/// several learns them all at once. A refusal writes nothing.
///
/// Member `i` of `component` is the knot's node `i`; the caller binds each member's slot to the
/// member at that node. A function member is born under the arena node `build`, and every member
/// keeps what it holds of its written code as `written` holds it.
#[allow(clippy::too_many_arguments)]
pub fn tie<'graph, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, '_>,
    component: &Component<'_>,
    born: (ArenaRef, Option<Stamp<'cell, Knotted<'graph, 'cell>>>),
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
    eager: &mut Eager<'_, 'graph, 'cell>,
) -> Result<Tied<'graph, 'cell, 'x>, Untieable<'x>>
where
    'graph: 'x,
{
    debug_assert!(
        component.deferred_only,
        "the shape builder refuses a component with an eager cycle"
    );
    let shape = activation.shape();
    let names = |owner: u32| shape.slot_name(component.members[owner as usize]);
    let mut roots: BumpVec<'x, Option<&ExpressionPart<'_>>> =
        BumpVec::with_capacity_in(component.members.len(), scratch);
    for slot in component.members {
        match shape.births(*slot).map(BodyShape::kind) {
            // A module is alone in its component and shares nothing with the staging below.
            Some(ShapeKind::Module) => {
                let knot = module::tie_member(
                    writer, activation, written, component, types, scratch, eager,
                )?;
                return Ok(Tied {
                    knot,
                    annotations: BumpVec::new_in(scratch),
                });
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
    let mut bodies = BumpVec::with_capacity_in(nodes.len(), scratch);
    bodies.extend(nodes.iter().enumerate().map(|(index, node)| match node {
        None => {
            let slot = component.members[index];
            shape
                .births(slot)
                .map(|body| (body, shape.registration(slot)))
        }
        Some(node) => node.function().map(|body| (body, None)),
    }));
    // Each member's annotation, resolved as the slot that binds it resolves it: a member is born
    // at it, so a fellow's edge into the node sees what the slot holds.
    let mut annotations: BumpVec<'x, Option<KType>> =
        BumpVec::with_capacity_in(component.members.len(), scratch);
    annotations.extend(component.members.iter().map(|slot| {
        let annotation = shape.annotation(*slot)?;
        let facts = activation.facts();
        let site = activation.shape().site(annotation);
        facts.denoted(types, activation.shape(), site).ok()
    }));
    // What each annotation re-stamps a member at, where the load recorded it.
    let mut retypes: BumpVec<'x, Option<Retype>> =
        BumpVec::with_capacity_in(component.members.len(), scratch);
    retypes.extend(component.members.iter().map(|slot| {
        let annotation = shape.annotation(*slot)?;
        let facts = activation.facts();
        let site = activation.shape().site(annotation);
        facts.retype(types, activation.shape(), site)
    }));
    let mut functions = function::stage(&plan, activation, written, &bodies, types, scratch)?;
    // An annotated function member is born at the annotation's retype, keeping the signature its
    // body was checked against for its frame.
    for ((staged, annotation), retype) in functions.iter_mut().zip(&annotations).zip(&retypes) {
        if let (Some(staged), Some(annotation)) = (staged.as_mut(), *annotation)
            && satisfied_by(types, scratch, annotation, staged.ktype)
        {
            let born = retype_target_as(types, scratch, staged.ktype, annotation, *retype);
            if born != staged.ktype {
                staged.declared = Some(staged.ktype);
                staged.ktype = born;
            }
        }
    }
    let mut codes = BumpVec::with_capacity_in(nodes.len(), scratch);
    codes.extend(nodes.iter().map(|node| {
        let (site, shape) = node.as_ref()?.code()?;
        let body = written.quote(activation.segment(), site);
        Some(code::staged(
            (body, written.root()),
            shape,
            activation,
            scratch,
            |index| {
                plan.edge(index)
                    .expect("a member index is below the knot's count")
            },
        ))
    }));
    // An annotated code member is born at the annotation's retype, as a function member is.
    for ((staged, annotation), retype) in codes.iter_mut().zip(&annotations).zip(&retypes) {
        if let (Some(staged), Some(annotation)) = (staged.as_mut(), *annotation)
            && satisfied_by(types, scratch, annotation, staged.ktype)
        {
            let born = retype_target_as(types, scratch, staged.ktype.into(), annotation, *retype);
            staged.ktype = born.as_type().expect("a quote's code is typed by a type");
        }
    }

    let mut memos: BumpVec<'x, Option<DeclaredType<KType>>> =
        BumpVec::with_capacity_in(nodes.len(), scratch);
    memos.extend(functions.iter().zip(&codes).map(|(function, code)| {
        function
            .as_ref()
            .map(|staged| staged.ktype)
            .or_else(|| code.as_ref().map(|staged| staged.ktype.into()))
    }));
    data::memos(
        &nodes,
        &mut memos,
        (&annotations, &retypes),
        &names,
        types,
        scratch,
    )?;
    data::check(&nodes, &memos, &names, types, scratch)?;

    let mut knot_weight = Weight::flat::<usize>();
    // Each function node's typing record (its quantifier map and registered shape) is laid down
    // once, beside its closure: it lives in the region for the knot's life, and a copy re-homes it
    // through the destination writer.
    let mut laid = BumpVec::with_capacity_in(functions.len(), scratch);
    for staged in functions.iter() {
        laid.push(staged.as_ref().map(|staged| {
            let (closure, typing, weight) = staged.laid_down(writer, born);
            knot_weight = knot_weight.plus(weight);
            (closure, typing)
        }));
    }
    let mut bound = BumpVec::with_capacity_in(codes.len(), scratch);
    for staged in codes.iter() {
        bound.push(staged.as_ref().map(|staged| {
            let (run, weight) = staged.laid_down(writer);
            knot_weight = knot_weight.plus(weight);
            run
        }));
    }
    // Every list node a node spreads is laid before it.
    let mut nodes_laid = BumpVec::with_capacity_in(nodes.len(), scratch);
    nodes_laid.resize(nodes.len(), None);
    for index in data::lay_order(&nodes, scratch) {
        let Some(node) = nodes[index]
            .as_ref()
            .filter(|node| node.function().is_none() && node.code().is_none())
        else {
            continue;
        };
        let memo = memos[index]
            .expect("every node's memo is derived")
            .as_type()
            .expect("a data node's memo is concrete");
        let data = data::lay_down(writer, node, memo, (&plan, &nodes_laid), types, scratch);
        knot_weight = knot_weight.plus(data.0.weight());
        nodes_laid[index] = Some(data);
    }
    let mut circulars = BumpVec::with_capacity_in(nodes.len(), scratch);
    circulars.extend(
        nodes_laid
            .iter()
            .map(|data| data.map(|(circular, _)| circular)),
    );
    knot_weight = (0..nodes.len()).fold(knot_weight, |weight, _| {
        weight.plus(Weight::flat::<Node<'graph, 'cell>>())
    });
    let knot = plan.tie(writer, |edge| {
        let index = edge.index() as usize;
        if let Some((staged, run)) = codes[index].as_ref().zip(bound[index]) {
            return staged.tied(writer, run, knot_weight);
        }
        match (functions[index].as_ref().zip(laid[index]), circulars[index]) {
            (Some((staged, (closure, typing))), _) => Node::Function(Function::new(
                staged.ktype,
                typing,
                staged.written_body(),
                closure,
                knot_weight,
            )),
            (None, Some(circular)) => Node::Data {
                circular,
                knot_weight: Cell::new(knot_weight),
            },
            (None, None) => unreachable!("every node is a function, a code or a data node"),
        }
    });
    Ok(Tied {
        knot: super::stamped(knot, types),
        annotations,
    })
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

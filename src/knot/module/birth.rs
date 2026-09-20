//! A module binder coming into being: the activation its body runs in, and the tie of the binder
//! once that body has bound every slot.
//!
//! Birth is body-first. The caller builds the body's activation with [`body_activation`], runs it
//! until every slot is bound, and only then ties the binder: a module's type and weight are facts
//! about the members its body binds, and a knot node is written once.
//!
//! **Layout order** is what makes the tie a read-out rather than a remap: a body shape lays its
//! slots out exactly the way a signature's tables are sorted ([`layout`](super::layout)), so a
//! body-born module's member run is its finished activation's slots read out in slot order.

use crate::elaborate::{Unsigned, self_signature};
use crate::knot::{KActivation, Node, Supplied, Untieable};
use crate::memory::{BumpAllocator, BumpVec, Knot, Writer};
use crate::scope::{
    Activation, Binding, ClosureBindings, ClosureRefused, Component, ShapeKind, Site, Slot,
};
use crate::type_lattice::TypeRegistry;

use super::Module;

/// The activation `slot`'s module body runs in: its captures read from `enclosing` and laid down,
/// every slot empty. The caller claims and binds its slots, then ties the binder with the finished
/// activation. A refusal writes nothing.
pub fn body_activation<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    enclosing: &KActivation<'graph, 'cell>,
    slot: Slot,
    scratch: BumpAllocator<'x>,
) -> Result<KActivation<'graph, 'cell>, Untieable<'x>> {
    let body = enclosing
        .shape()
        .births(slot)
        .expect("a module binder births its body");
    debug_assert_eq!(body.kind(), ShapeKind::Module);
    let captures = ClosureBindings::read_captures(body, enclosing, scratch, |_| {
        unreachable!("a module captures no fellow member: it is alone in its component")
    })
    .map_err(|ClosureRefused { name, pending }| Untieable::Pending {
        name,
        binder: pending,
    })?;
    let closure = ClosureBindings::of(writer, &captures);
    Ok(Activation::of_module(
        writer,
        body,
        closure,
        enclosing.builtins(),
    ))
}

/// Tie the lone module member of `component`: the caller's supplied body, read out in slot order.
///
/// A module is alone in its component and shares nothing with the staging the
/// [tie](crate::knot::tie()) does for the other member kinds, so it ties on its own path.
pub fn tie_member<'graph, 'cell, 'x>(
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
    Ok(Module::tie(writer, ktype, &members))
}

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

use crate::elaborate::self_signature;
use crate::knot::{Eager, KActivation, KActivationView, Node, Supplied, Untieable};
use crate::memory::{BumpAllocator, BumpVec, Knot, Writer};
use crate::scope::{Activation, ClosureBindings, Component, ShapeKind, Slot};
use crate::type_lattice::TypeRegistry;

use super::Module;

/// The activation `slot`'s module body runs in: its captures read from `enclosing` and laid down,
/// every slot empty. The caller binds its slots, then ties the binder with the finished
/// activation.
pub fn body_activation<'graph, 'cell>(
    writer: Writer<'cell>,
    enclosing: &KActivationView<'graph, 'cell>,
    slot: Slot,
    scratch: BumpAllocator<'_>,
) -> KActivation<'graph, 'cell> {
    let body = enclosing
        .shape()
        .births(slot)
        .expect("a module binder births its body");
    debug_assert_eq!(body.kind(), ShapeKind::Module);
    let captures = ClosureBindings::read_captures(body, enclosing, scratch, |_| {
        unreachable!("a module captures no fellow member: it is alone in its component")
    });
    let closure = ClosureBindings::of(writer, &captures);
    Activation::of_module(writer, body, closure, enclosing.builtins())
}

/// Tie the lone module member of `component`: the caller's supplied body, read out in slot order.
///
/// A module is alone in its component and shares nothing with the staging the
/// [tie](crate::knot::tie()) does for the other member kinds, so it ties on its own path.
pub fn tie_member<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    activation: &KActivationView<'graph, 'cell>,
    component: &Component<'graph>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
    eager: &mut Eager<'_, 'graph, 'cell>,
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
    let body = match eager(site, None) {
        Some(Supplied::Body(body)) => body,
        Some(Supplied::Value(_)) => unreachable!("a module member is asked for its run body"),
        None => return Err(Untieable::Eager { name, site }),
    };
    debug_assert!(std::ptr::eq(
        body.shape(),
        shape.births(*slot).expect("this binder births its body"),
    ));
    let ktype = self_signature(body, types, scratch);
    let mut members = BumpVec::with_capacity_in(body.shape().slots(), scratch);
    members.extend(body.slots().map(|(_, value)| value));
    Ok(Module::tie(writer, ktype, &members))
}

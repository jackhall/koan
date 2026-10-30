//! A module binder coming into being: the activation its body runs in, and the tie of the binder
//! once that body has bound every slot.
//!
//! Birth is body-first. The caller builds the body's activation with [`body_activation`], runs it
//! until every slot is bound, and only then ties the binder: a module's type and weight are facts
//! about the members its body binds, and a knot node is written once.
//!
//! **Layout order**: the signature places each member ([`elaborate`'s members](crate::elaborate)),
//! so the tie places each named slot by its name, at the index the module's own signature gives
//! it, and lays the registrations after them in slot order. Nothing assumes the body's slot order.

use crate::elaborate::{member_count, schema_member, schema_of, self_signature};
use crate::knot::{Eager, KActivation, KReading, Knotted, Node, Supplied, Untieable};
use crate::memory::{BumpAllocator, BumpVec, Knot, Writer};
use crate::scope::{
    Activation, ClosureBindings, Component, Coordinate, ShapeKind, Slot, Target, Written,
};
use crate::symbols::BinderSymbol;
use crate::values::{ArenaRef, Chain};

use super::{Born, Module};

/// The activation `slot`'s module body runs in: its captures read from `enclosing` and laid down,
/// every slot empty. The caller binds its slots, then ties the binder with the finished
/// activation.
pub fn body_activation<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    enclosing: &KReading<'_, 'graph, 'cell>,
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
    // A module is never called, so its closure seeks no settlement. The module keeps it, to
    // compute its content from where a demand first reaches it.
    let closure = ClosureBindings::of(writer, &captures, (ArenaRef::ROOT, None), None);
    Activation::of_module(
        writer,
        body,
        enclosing.facts(),
        closure,
        enclosing.builtins(),
        enclosing.view().decided(),
    )
}

/// Tie the lone module member of `component`: the caller's supplied body, each slot placed where
/// the module's signature places its name.
///
/// A module is alone in its component and shares nothing with the staging the
/// [tie](crate::knot::tie()) does for the other member kinds, so it ties on its own path. The
/// module keeps its body's record as `written` holds it.
pub fn tie_member<'graph, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, 'cell, '_>,
    component: &Component<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
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
        Some(Supplied::Body(body)) => activation.beside(body.view()),
        Some(Supplied::Value(_)) => unreachable!("a module member is asked for its run body"),
        None => return Err(Untieable::Eager { name, site }),
    };
    debug_assert!(std::ptr::eq(
        body.shape(),
        shape.births(*slot).expect("this binder births its body"),
    ));
    let mut keyworded = BumpVec::with_capacity_in(body.shape().registrations().len(), scratch);
    for registration in body.shape().registrations() {
        let member = body
            .read(Coordinate::Activation {
                hops: 0,
                target: Target::Local(registration.slot),
            })
            .as_callable()
            .and_then(Knotted::function)
            .and_then(|function| function.registered_shape())
            .expect("a registration binds the function born for it");
        keyworded.push(member);
    }
    let ktype = self_signature(&body, &keyworded, types, scratch);
    // The signature places each named slot by its name; the registrations follow in slot order.
    let schema = schema_of(ktype, types).expect("a module's type is its signature");
    let mut placed = BumpVec::with_capacity_in(member_count(&schema, scratch), scratch);
    placed.resize(member_count(&schema, scratch), None);
    let mut registrations = BumpVec::with_capacity_in(keyworded.len(), scratch);
    for (slot, value) in body.slots() {
        match body.shape().slot_name(slot) {
            BinderSymbol::Registration(_) => registrations.push(value),
            name => {
                let (index, _) = schema_member(&schema, scratch, name)
                    .expect("a module's signature names every slot its body binds");
                placed[index] = Some(value);
            }
        }
    }
    let mut members = BumpVec::with_capacity_in(body.shape().slots(), scratch);
    members.extend(
        (placed.into_iter())
            .map(|value| value.expect("a module's body binds every member its signature names")),
    );
    members.extend(registrations);
    // A body-born module is its code over what it captures, digested where a demand first
    // reaches it: the module keeps its body's closure until then.
    let born = Born::Body {
        body: written.body(writer, body.segment()),
        closure: body.closure(),
    };
    Ok(Module::tie(writer, ktype, &members, born, types))
}

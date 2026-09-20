//! A module as a knot node: its self-signature, its members in layout order, and the activation
//! its body runs in.
//!
//! **Layout order** is the one rule this item shares end to end: value members sorted by name,
//! then type members sorted by name. A body shape lays its slots out exactly that way and a
//! signature's tables are sorted by the same `Ord`, so a body-born module's member run is its
//! finished activation's slots read out in slot order, with no remap.
//!
//! Birth is body-first. The caller builds the body's activation with [`module_activation`], runs
//! it until every slot is bound, and only then ties the binder: a module's type and weight are
//! facts about the members its body binds, and a knot node is written once. The module door here
//! and the view door one layer above both lay a node down through [`module`], so a body-born
//! module and a view have one representation and one copy.

use crate::memory::{BumpAllocator, Knot, KnotPlan, Writer, collect};
use crate::scope::{Activation, ClosureBindings, ClosureRefused, ShapeKind, Slot};
use crate::type_lattice::KType;
use crate::values::Weight;

use super::{KActivation, KValue, Node, Untieable};

/// A module: what one knot node holds.
pub struct Module<'graph, 'cell> {
    ktype: KType,
    /// The bound members, in layout order.
    members: &'cell [KValue<'graph, 'cell>],
    /// What rebuilding the whole knot this node sits in writes, the same on every node.
    knot_weight: Weight,
}

impl Clone for Module<'_, '_> {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for Module<'_, '_> {}

impl<'graph, 'cell> Module<'graph, 'cell> {
    /// The module's self-signature, or the signature a view was built at.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The members, in layout order.
    pub fn members(&self) -> &'cell [KValue<'graph, 'cell>] {
        self.members
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight
    }
}

/// A module over `members`, already in layout order under `ktype`, laid down as a one-node knot in
/// `writer`'s region. The one private-field constructor the tie and the view door share; the module
/// is node `0`.
///
/// A member that is itself a knot member carries its whole knot's weight, since a crossing rebuilds
/// that knot whole.
pub fn module<'graph, 'cell>(
    writer: Writer<'cell>,
    ktype: KType,
    members: &[KValue<'graph, 'cell>],
) -> Knot<'cell, Node<'graph, 'cell>> {
    let knot_weight = members.iter().fold(
        Weight::flat::<usize>().plus(Weight::flat::<Node<'graph, 'cell>>()),
        |weight, member| weight.plus(member.weight()),
    );
    let members = collect(writer, members.iter().copied());
    KnotPlan::new(1).tie(writer, |_| {
        Node::Module(Module {
            ktype,
            members,
            knot_weight,
        })
    })
}

/// The activation `slot`'s module body runs in: its captures read from `enclosing` and laid down,
/// every slot empty. The caller claims and binds its slots, then ties the binder with the finished
/// activation. A refusal writes nothing.
pub fn module_activation<'graph, 'cell, 'x>(
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

impl<'graph, 'cell> Module<'graph, 'cell> {
    /// This module over `members` rebuilt at another region lifetime — the copy's arm. The
    /// signature and the knot weight are facts about the members, which the copy preserves.
    pub(super) fn rebuilt<'to>(&self, members: &'to [KValue<'graph, 'to>]) -> Module<'graph, 'to> {
        Module {
            ktype: self.ktype,
            members,
            knot_weight: self.knot_weight,
        }
    }
}

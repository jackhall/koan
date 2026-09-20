//! Modules as values: the node a module is, the views `:|` and `:!` build, the coercion that
//! births a view's members, and the binding a `USING … SCOPE` block enters on.
//!
//! A module's node holds its self-signature, its members in layout order and its knot weight —
//! never an activation, since a view has no body to activate. A module is always a one-node knot:
//! a mention reached from a module binder's root is eager whatever body it sits in. [`birth`] is
//! where a body-born one comes from, [`Module::tie`] the one private-field door a body-born module
//! and a view both go through, so there is one representation and one copy.
//!
//! Everything else here reads a module by name. Its spine is **layout order** — value members
//! sorted by name, then type members sorted by name — which a body shape's slots, a signature's
//! tables and a `USING` block's parameters all agree on, so a member is reached by index and never
//! by a search through the value it sits in. [`layout`] owns that rule; nothing else here spells it.
//!
//! A **view** narrows: [`view::ascribe`] checks the source satisfies the signature, keeps only what
//! the signature names, and lays a module node of its own down through the same door. Under `:!`
//! the view's types are the source's, so every member is carried verbatim. Under `:|` each abstract
//! member is minted afresh per application, and every member is born **coerced** to the mint: data
//! is re-tagged through the admission barrier, containers are rebuilt cell by cell, a nested module
//! is re-viewed, and a function is wrapped in a [`Coerced`] barrier node a call will later go
//! through.
//!
//! What this layer does *not* do is evaluate anything: `m :| Sig`, `m.f`, a `USING` expression and
//! a call through a barrier are all [modules](../../roadmap/rewrite/modules.md)' work. Here are the
//! doors those will drive.
//!
//! This module names its [knot vocabulary](crate::knot) through the facade and never its sibling
//! [`function`](super::function).
//!
//! See [module/README.md](module/README.md).

mod birth;
pub mod coerce;
pub mod layout;
pub mod surface;
pub mod view;

#[cfg(test)]
mod tests;

pub use birth::{body_activation, tie_member};

use crate::memory::{Knot, KnotPlan, Writer, collect, resident};
use crate::type_lattice::KType;
use crate::values::{Knotted as _, Weight};

use super::{KValue, Knotted, Node};

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
    /// A module over `members`, already in layout order under `ktype`, laid down as a one-node knot
    /// in `writer`'s region. The one private-field constructor the tie and the view door share; the
    /// module is node `0`.
    ///
    /// A member that is itself a knot member carries its whole knot's weight, since a crossing
    /// rebuilds that knot whole.
    pub fn tie(
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

/// A function member behind an opaque view's barrier: what one knot node points at.
///
/// A call goes through the barrier — coercing its arguments inwards and its return outwards —
/// before `underlying` runs; that is [modules](../../roadmap/rewrite/modules.md)' work, not this
/// item's, which only gives the barrier somewhere to live.
pub struct Coerced<'graph, 'cell> {
    underlying: Knotted<'graph, 'cell>,
    ktype: KType,
    declared: KType,
    from: KType,
    to: KType,
    /// What rebuilding the whole knot this node sits in writes, the same on every node.
    knot_weight: Weight,
}

impl Clone for Coerced<'_, '_> {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for Coerced<'_, '_> {}

impl<'graph, 'cell> Coerced<'graph, 'cell> {
    /// A function member behind an opaque view's barrier, laid down as a one-node knot in
    /// `writer`'s region; the member is node `0`. Its six fields sit in a resident struct the node
    /// points at rather than in the node itself, so the arm is one word wide and the module arm
    /// keeps setting the node's width.
    ///
    /// `underlying` carries its whole knot's weight, since a crossing rebuilds that knot whole.
    pub fn tie(
        writer: Writer<'cell>,
        underlying: Knotted<'graph, 'cell>,
        ktype: KType,
        declared: KType,
        from: KType,
        to: KType,
    ) -> Knot<'cell, Node<'graph, 'cell>> {
        debug_assert!(
            underlying.function().is_some() || underlying.coerced().is_some(),
            "only a function, or a function already behind a barrier, is coerced"
        );
        let knot_weight = Weight::flat::<usize>()
            .plus(Weight::flat::<Node<'graph, 'cell>>())
            .plus(Weight::flat::<Coerced<'graph, 'cell>>())
            .plus(underlying.weight());
        KnotPlan::new(1).tie(writer, |_| {
            Node::Coerced(resident(
                writer,
                Coerced {
                    underlying,
                    ktype,
                    declared,
                    from,
                    to,
                    knot_weight,
                },
            ))
        })
    }

    /// The function the call runs once it is through the barrier — itself a function or a coerced
    /// member, since a view of a view stacks barriers.
    pub fn underlying(&self) -> Knotted<'graph, 'cell> {
        self.underlying
    }

    /// The function type at the view's substitution: what a caller sees.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The slot type the view's signature declares, which the coercion walk recurses on.
    pub fn declared(&self) -> KType {
        self.declared
    }

    /// A `Signature` handle whose manifest members are the source module's bindings.
    pub fn from(&self) -> KType {
        self.from
    }

    /// A `Signature` handle whose manifest members are the view's bindings.
    pub fn to(&self) -> KType {
        self.to
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight
    }

    /// This barrier over `underlying` rebuilt at another region lifetime — the copy's arm. The
    /// types and the knot weight are facts about what sits behind the barrier, which the copy
    /// preserves.
    pub(super) fn rebuilt<'to>(&self, underlying: Knotted<'graph, 'to>) -> Coerced<'graph, 'to> {
        Coerced {
            underlying,
            ktype: self.ktype,
            declared: self.declared,
            from: self.from,
            to: self.to,
            knot_weight: self.knot_weight,
        }
    }
}

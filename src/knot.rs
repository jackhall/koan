//! The knot layer: functions, modules and circular data as values — what closes
//! [`Value`](crate::values::Value)'s knot-member parameter.
//!
//! A knot member is one node of a [`Knot`](crate::memory::Knot) laid down in a cell's region.
//! [`Knotted`] is that node, the `(knot, index)` pair, and [`Node`] is what one holds: a
//! [function](crate::knot::function), a data node — a [`Circular`] over links — a [module](crate::knot::module), or a
//! barrier over a function member of an opaque view. A function that names no fellow is a one-node
//! knot, and so is every module and every barrier; a deferred-only component of value binders is
//! born together as one knot by [`tie`], each mention of a fellow member an edge into it.
//!
//! A module's node is the carrier, not a claim about cycles: `m.f` is not a knot edge but an index
//! into the run, and the run holds members of knots the module does not own. A mention reached from
//! a module binder's root is eager whatever body it sits in, so a module is never in a cycle.
//!
//! [`Knotted`] is sixteen bytes, so a value holding one stays one twenty-four-byte word. A member
//! copies at a crossing by re-tying its whole knot at the destination, each held value deep-copied
//! and each edge carried verbatim, priced by the knot's memoized weight under the ordinary verdict.
//! Structural equality over a function is [`Incomparable`](crate::values::Incomparable).
//!
//! **What sits where.** This file is the vocabulary every node kind shares: the member, the node,
//! the value and activation spelled at it, and the [`Supplied`] and [`Untieable`] a birth answers
//! with. [`function`] and [`module`] each own one node kind — its payload, its doors and,
//! for a module, everything that reads one by name — and neither names the other; [`data`] owns
//! the data node; [`tie`](crate::knot::tie()) births a component over them, and `copy` re-ties a whole knot.
//! A submodule reaches this vocabulary through the facade, never a sibling.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::elaborate`,
//! `crate::memory`, `crate::parse`, `crate::scope`, `crate::symbols`, `crate::type_lattice` and
//! `crate::values`, and nothing else in the crate; `tests::boundary` reads the source to hold it
//! there. Neither `values` nor `scope` names this module.
//!
//! See [knot/README.md](knot/README.md).

mod copy;
mod data;
pub mod function;
pub mod module;
mod tie;

#[cfg(test)]
pub(crate) mod tests;

pub use function::Function;
pub use module::{Coerced, Module};
pub use tie::tie;

use std::fmt;

use crate::elaborate::Elaboration;
use crate::memory::{DropFree, Edge, Member, covariant, reattachable};
use crate::parse::ExpressionPart;
use crate::scope::{Activation, ActivationView, Builtins, Site};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{
    self, Circular, ConstructionRefused, KeyRejected, Resolved, Value, ValueCarrier, ValueFamily,
    Weight,
};

/// The payload of a knot node.
#[derive(Clone, Copy)]
pub enum Node<'graph, 'cell> {
    Function(Function<'graph, 'cell, Knotted<'graph, 'cell>>),
    /// A data node: a member's right-hand side, or an anonymous constructor below one on the path to
    /// a sibling mention.
    Data {
        circular: Circular<'graph, 'cell, Knotted<'graph, 'cell>>,
        /// What rebuilding the whole knot this node sits in writes, the same on every node.
        knot_weight: Weight,
    },
    /// A module: its self-signature, its members in layout order, and the same weight.
    Module(Module<'graph, 'cell>),
    /// A function member behind an opaque view's barrier: the barrier sits beside the node in the
    /// same region and the node points at it, since its six fields would otherwise be the widest
    /// arm and every node in the program pays for that.
    Coerced(&'cell Coerced<'graph, 'cell>),
}

const _: () = assert!(!std::mem::needs_drop::<Node<'static, 'static>>());

/// A knot member: one node of a knot — a function, a data node, a module or a barrier over a
/// function. Its equality and hash are node identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Knotted<'graph, 'cell>(Member<'cell, Node<'graph, 'cell>>);

const _: () = assert!(size_of::<Knotted<'static, 'static>>() == 16);
const _: () = assert!(size_of::<KValue<'static, 'static>>() == 24);
const _: () = assert!(size_of::<KActivation<'static, 'static>>() == 80);
/// The module arm sets the node's width; a barrier's six fields would widen every node in the
/// program, so [`Node::Coerced`] points at them instead.
const _: () = assert!(size_of::<Node<'static, 'static>>() == 64);

impl<'graph, 'cell> Knotted<'graph, 'cell> {
    /// The knot node this member is.
    pub fn member(self) -> Member<'cell, Node<'graph, 'cell>> {
        self.0
    }

    pub fn node(self) -> &'cell Node<'graph, 'cell> {
        self.0.payload()
    }

    /// The function this member runs, if it is a function's node.
    pub fn function(self) -> Option<&'cell Function<'graph, 'cell, Self>> {
        match self.node() {
            Node::Function(function) => Some(function),
            _ => None,
        }
    }

    /// The module this member is, if it is a module's node.
    pub fn module(self) -> Option<&'cell Module<'graph, 'cell>> {
        match self.node() {
            Node::Module(module) => Some(module),
            _ => None,
        }
    }

    /// The barrier this member sits behind, if it is a coerced function's node.
    pub fn coerced(self) -> Option<&'cell Coerced<'graph, 'cell>> {
        match self.node() {
            Node::Coerced(coerced) => Some(coerced),
            _ => None,
        }
    }
}

impl fmt::Debug for Knotted<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Knotted")
            .field("knot", &self.0.knot().len())
            .field("index", &self.0.index().index())
            .finish()
    }
}

impl values::Knotted for Knotted<'_, '_> {
    fn ktype(&self) -> KType {
        match self.node() {
            Node::Function(function) => function.ktype(),
            Node::Data { circular, .. } => circular.ktype(),
            Node::Module(module) => module.ktype(),
            Node::Coerced(coerced) => coerced.ktype(),
        }
    }

    fn weight(&self) -> Weight {
        match self.node() {
            Node::Function(function) => function.knot_weight(),
            Node::Data { knot_weight, .. } => *knot_weight,
            Node::Module(module) => module.knot_weight(),
            Node::Coerced(coerced) => coerced.knot_weight(),
        }
    }

    fn sibling(&self, edge: Edge) -> Self {
        Knotted(self.0.follow(edge))
    }

    fn resolve<'a>(&self) -> Resolved<'a, Self>
    where
        Self: 'a,
    {
        match self.node() {
            Node::Function(_) => Resolved::Function,
            Node::Data { circular, .. } => Resolved::Circular(*circular),
            Node::Module(_) => Resolved::Module,
            // A barrier is as opaque to `values` as the function behind it, and calls the same way.
            Node::Coerced(_) => Resolved::Function,
        }
    }
}

/// What the caller supplies for a part only it can produce: an evaluated value for a data member's
/// part, or a module binder's body already run.
#[derive(Clone, Copy)]
pub enum Supplied<'graph, 'cell> {
    Value(KValue<'graph, 'cell>),
    /// A module binder's body, run: its activation with every slot bound.
    Body(&'cell KActivation<'graph, 'cell>),
}

/// What a tie's caller answers a part only it can evaluate with, asked by site with the part itself
/// — or `None` for a module body, which the caller runs.
pub type Eager<'e, 'graph, 'cell> =
    dyn FnMut(Site, Option<&'graph ExpressionPart<'graph>>) -> Option<Supplied<'graph, 'cell>> + 'e;

/// Why a component could not be tied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untieable<'x> {
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

/// The family of [`Knotted`], which `values` crosses a callable through.
pub struct KnottedFamily;

reattachable!(KnottedFamily => Knotted<'graph, 'cell>);

impl DropFree for KnottedFamily {}

/// A value that may hold a function.
pub type KValue<'graph, 'cell> = Value<'graph, 'cell, Knotted<'graph, 'cell>>;

/// The family of [`KValue`].
pub type KValueFamily = ValueFamily<KnottedFamily>;

// A value crosses between cells, so its family carries the covariance witness the crossing doors
// ask for.
covariant!(KValueFamily);

/// A carrier of a [`KValue`].
pub type KValueCarrier<'graph, 'home> = ValueCarrier<'graph, 'home, KnottedFamily>;

/// An activation whose values may hold functions.
pub type KActivation<'graph, 'cell> = Activation<'graph, 'cell, KnottedFamily>;

/// The read half of a [`KActivation`]. The member is spelled out rather than left to the view's
/// default: a type holding one in a field is then covariant in `'cell`, which the projection the
/// default names would not be.
pub type KActivationView<'graph, 'cell> =
    ActivationView<'graph, 'cell, KnottedFamily, Knotted<'graph, 'cell>>;

/// A builtin table whose values may hold functions.
pub type KBuiltins<'graph, 'cell> = Builtins<'graph, 'cell, Knotted<'graph, 'cell>>;

//! Functions and circular data as values: the layer that closes
//! [`Value`](crate::values::Value)'s knot-member parameter.
//!
//! A knot member is one node of a [`Knot`](crate::memory::Knot) laid down in a cell's region. A
//! function's node holds its type, the body shape it runs, its closure bindings — the scope layer's
//! run of links — and the rebuild weight of the whole knot it sits in; a data node holds a
//! [`Circular`](crate::values::Circular) over links and the same weight; a module's node holds its
//! self-signature, its members in layout order and the same weight. A function that names no
//! fellow is a one-node knot, and so is every module — a mention reached from a module binder's
//! root is eager whatever body it sits in, so a module is never in a cycle. A deferred-only
//! component of value binders is born together as one knot by [`tie`], each mention of a fellow
//! member an edge into it.
//!
//! A module's node is the carrier, not a claim about cycles: `m.f` is not a knot edge but an index
//! into the run, and the run holds members of knots the module does not own. A fourth node holds a
//! function member of an opaque view: a barrier the module layer lays down over the function it
//! coerces, and another one-node knot.
//!
//! [`Knotted`] is sixteen bytes, so a value holding one stays one twenty-four-byte word. A member
//! copies at a crossing by re-tying its whole knot at the destination, each held value deep-copied
//! and each edge carried verbatim, priced by the knot's memoized weight under the ordinary verdict.
//! Structural equality over a function is [`Incomparable`](crate::values::Incomparable).
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::elaborate`,
//! `crate::memory`, `crate::parse`, `crate::scope`, `crate::type_lattice` and `crate::values`, and
//! nothing else in the crate; `tests::boundary` reads the source to hold it there. Neither `values`
//! nor `scope` names this module.
//!
//! See [function/README.md](function/README.md).

mod birth;
mod copy;
mod data;
mod module;

#[cfg(test)]
pub(crate) mod tests;

pub use birth::{Supplied, Untieable, tie};
pub use module::{Coerced, Module, coerced, module, module_activation};

use std::fmt;

use crate::memory::{DropFree, Edge, Member, reattachable};
use crate::scope::{Activation, BodyShape, ClosureBindings};
use crate::type_lattice::KType;
use crate::values::{self, Circular, Resolved, Value, ValueCarrier, ValueFamily, Weight};

/// A function: what one knot node holds.
pub struct Function<'graph, 'cell, X> {
    ktype: KType,
    shape: &'graph BodyShape<'graph>,
    closure: &'cell ClosureBindings<'graph, 'cell, X>,
    /// What rebuilding the whole knot this node sits in writes, the same on every node.
    knot_weight: Weight,
}

impl<X> Clone for Function<'_, '_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X> Copy for Function<'_, '_, X> {}

impl<'graph, 'cell, X> Function<'graph, 'cell, X> {
    /// The function's type, elaborated from its signature where it was born.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The body shape a call activates.
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    /// The closure bindings a call's activation reads its captures through.
    pub fn closure(&self) -> &'cell ClosureBindings<'graph, 'cell, X> {
        self.closure
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight
    }
}

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
const _: () = assert!(size_of::<KActivation<'static, 'static>>() == 72);
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
            Node::Function(function) => function.ktype,
            Node::Data { circular, .. } => circular.ktype(),
            Node::Module(module) => module.ktype(),
            Node::Coerced(coerced) => coerced.ktype(),
        }
    }

    fn weight(&self) -> Weight {
        match self.node() {
            Node::Function(function) => function.knot_weight,
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

/// The family of [`Knotted`], which `values` crosses a callable through.
pub struct KnottedFamily;

reattachable!(KnottedFamily => Knotted<'graph, 'cell>);

impl DropFree for KnottedFamily {}

/// A value that may hold a function.
pub type KValue<'graph, 'cell> = Value<'graph, 'cell, Knotted<'graph, 'cell>>;

/// The family of [`KValue`].
pub type KValueFamily = ValueFamily<KnottedFamily>;

/// A carrier of a [`KValue`].
pub type KValueCarrier<'graph, 'home> = ValueCarrier<'graph, 'home, KnottedFamily>;

/// An activation whose values may hold functions.
pub type KActivation<'graph, 'cell> = Activation<'graph, 'cell, Knotted<'graph, 'cell>>;

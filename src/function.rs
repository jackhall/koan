//! Functions as values: the layer that closes [`Value`](crate::values::Value)'s callable parameter.
//!
//! A callable is one node of a [`Knot`](crate::memory::Knot) laid down in a cell's region. A
//! function's node holds its type, the body shape it runs, its closure bindings — the scope layer's
//! run of value words and edges — and the rebuild weight of the whole knot it sits in. A function
//! that names no fellow is a one-node knot; a strongly connected component of callable binders is
//! born together as one knot by [`tie`], each capture of a fellow member an edge into it.
//!
//! [`Callable`] is sixteen bytes — a knot member — so a value holding one stays one twenty-four-byte
//! word. A callable copies at a crossing by re-tying its whole knot at the destination, each closure
//! binding deep-copied and each edge carried verbatim, priced by the knot's memoized weight under
//! the ordinary verdict. Structural equality over a callable is
//! [`Incomparable`](crate::values::Incomparable).
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::elaborate`,
//! `crate::memory`, `crate::parse`, `crate::scope`, `crate::type_lattice` and `crate::values`, and
//! nothing else in the crate; `tests::boundary` reads the source to hold it there. Neither `values`
//! nor `scope` names this module.

mod birth;
mod copy;

#[cfg(test)]
mod tests;

pub use birth::{Untieable, tie};

use std::fmt;

use crate::memory::{DropFree, Edge, Member, reattachable};
use crate::parse::LabelInterner;
use crate::scope::{Activation, ClosureBindings, Shape};
use crate::type_lattice::{KType, TypeRegistry, display_name};
use crate::values::{self, Value, ValueCarrier, ValueFamily, Weight};

/// A function: what one knot node holds.
pub struct Function<'graph, 'cell, X> {
    ktype: KType,
    shape: &'graph Shape<'graph>,
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
    pub fn shape(&self) -> &'graph Shape<'graph> {
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
    Function(Function<'graph, 'cell, Callable<'graph, 'cell>>),
}

const _: () = assert!(!std::mem::needs_drop::<Node<'static, 'static>>());

/// The closed callable: one node of a knot.
#[derive(Clone, Copy)]
pub struct Callable<'graph, 'cell>(Member<'cell, Node<'graph, 'cell>>);

const _: () = assert!(size_of::<Callable<'static, 'static>>() == 16);
const _: () = assert!(size_of::<KValue<'static, 'static>>() == 24);

impl<'graph, 'cell> Callable<'graph, 'cell> {
    /// The knot node this callable is.
    pub fn member(self) -> Member<'cell, Node<'graph, 'cell>> {
        self.0
    }

    pub fn node(self) -> &'cell Node<'graph, 'cell> {
        self.0.payload()
    }

    /// The function this callable runs.
    pub fn function(self) -> &'cell Function<'graph, 'cell, Self> {
        match self.node() {
            Node::Function(function) => function,
        }
    }
}

impl fmt::Debug for Callable<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Callable")
            .field("knot", &self.0.knot().len())
            .field("index", &self.0.index().index())
            .finish()
    }
}

impl values::Callable for Callable<'_, '_> {
    fn ktype(&self) -> KType {
        self.function().ktype
    }

    fn weight(&self) -> Weight {
        self.function().knot_weight
    }

    /// A callable renders as its type does: its closure bindings are program state.
    fn render(
        &self,
        out: &mut impl fmt::Write,
        types: &TypeRegistry<'_>,
        labels: &LabelInterner,
    ) -> fmt::Result {
        write!(
            out,
            "{}",
            display_name(self.function().ktype, types, labels)
        )
    }

    fn sibling(&self, edge: Edge) -> Self {
        Callable(self.0.follow(edge))
    }
}

/// The family of [`Callable`], which `values` crosses a callable through.
pub struct CallableFamily;

reattachable!(CallableFamily => Callable<'graph, 'cell>);

impl DropFree for CallableFamily {}

/// A value that may hold a function.
pub type KValue<'graph, 'cell> = Value<'graph, 'cell, Callable<'graph, 'cell>>;

/// The family of [`KValue`].
pub type KValueFamily = ValueFamily<CallableFamily>;

/// A carrier of a [`KValue`].
pub type KValueCarrier<'graph, 'home> = ValueCarrier<'graph, 'home, CallableFamily>;

/// An activation whose values may hold functions.
pub type KActivation<'graph, 'cell> = Activation<'graph, 'cell, Callable<'graph, 'cell>>;

//! What an elaboration reads its type names through: the activation a type expression is read in,
//! or — for a check that runs where the program loads, before any activation exists — the builtin
//! table alone.
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and the type a coordinate's
//! binding holds. [`BuiltinsOnly`] reaches only builtin coordinates, so a signature naming anything
//! else does not elaborate through it.

use crate::scope::{Activation, ActivationView, BodyShape, Builtins, Coordinate};
use crate::type_lattice::KType;
use crate::values::{Knotted, KnottedFamily, Value};

/// A reader of the names a type expression mentions.
pub trait Reads<'graph> {
    /// The shape whose mentions the expression's names are.
    fn shape(&self) -> &'graph BodyShape<'graph>;

    /// The type the binding at `at` holds, or `None` where it holds something else or this reader
    /// cannot reach it.
    fn type_at(&self, at: Coordinate) -> Option<KType>;
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for ActivationView<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> Option<KType> {
        match self.read(at) {
            Value::Type(value) => Some(value.handle()),
            _ => None,
        }
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for Activation<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> Option<KType> {
        ActivationView::type_at(self, at)
    }
}

/// A reader that reaches the builtin table and nothing else: what the load-time overlap check
/// elaborates a registration's signature through, since no activation exists where it runs.
pub struct BuiltinsOnly<'e, 'graph, 'cell, X> {
    pub shape: &'graph BodyShape<'graph>,
    pub builtins: &'e Builtins<'cell, X>,
}

impl<'graph, X: Knotted> Reads<'graph> for BuiltinsOnly<'_, 'graph, '_, X> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    fn type_at(&self, at: Coordinate) -> Option<KType> {
        match at {
            Coordinate::Builtin(index) => match self.builtins.get(index) {
                Value::Type(value) => Some(value.handle()),
                _ => None,
            },
            _ => None,
        }
    }
}

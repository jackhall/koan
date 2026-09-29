//! What an elaboration reads its type names through: the activation a type expression is read in,
//! or — where the program loads, before any activation exists — the load pass's own reader
//! ([`channel`](super::channel)).
//!
//! Every name a type expression reads is a mention the shape builder resolved to a
//! [`Coordinate`]; a reader answers the shape holding those mentions and what a coordinate's
//! binding holds, as a [`TypeAt`]. An activation answers a type or not one; the load-time reader
//! may also answer a rigid variable standing for a type a run binds, or that it cannot know.

use crate::scope::{Activation, ActivationView, BodyShape, Builtins, Coordinate};
use crate::type_lattice::KType;
use crate::values::{Knotted, KnottedFamily, Value};

/// What a reader answers for the binding at a coordinate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeAt {
    /// It holds this type.
    Type(KType),
    /// A run binds it: this rigid variable stands for its type.
    Rigid(KType),
    /// It holds something other than a type.
    NotAType,
    /// This reader cannot know it before the program runs.
    Unknown,
}

/// A reader of the names a type expression mentions.
pub trait Reads<'graph> {
    /// The shape whose mentions the expression's names are.
    fn shape(&self) -> &'graph BodyShape<'graph>;

    /// What the binding at `at` holds.
    fn type_at(&self, at: Coordinate) -> TypeAt;

    /// How many [`TypeAt::Rigid`] answers this reader has given: what a spelling that must not be
    /// elaborated over a rigid variable compares before and after reading its operands.
    fn rigid_reads(&self) -> u32 {
        0
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for ActivationView<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match self.read(at) {
            Value::Type(value) => TypeAt::Type(value.handle()),
            _ => TypeAt::NotAType,
        }
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Reads<'graph> for Activation<'graph, '_, XF> {
    fn shape(&self) -> &'graph BodyShape<'graph> {
        ActivationView::shape(self)
    }

    fn type_at(&self, at: Coordinate) -> TypeAt {
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

    fn type_at(&self, at: Coordinate) -> TypeAt {
        match at {
            Coordinate::Builtin(index) => match self.builtins.get(index) {
                Value::Type(value) => TypeAt::Type(value.handle()),
                _ => TypeAt::NotAType,
            },
            _ => TypeAt::NotAType,
        }
    }
}

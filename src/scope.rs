//! Koan's lexical environments: what a name means at the point it is read, in three tiers.
//!
//! - The [`BodyShape`] — one per body, built once into program storage — declares the body's value and
//!   type names, classifies every mention eager or deferred, delimits the components its bindings
//!   form, and resolves every name the body reads to a [`Coordinate`].
//! - [`ClosureBindings`] — one run per callable, born from the enclosing activation through the
//!   shape's capture layout: a value word per capture, or an edge into the callable's own knot.
//! - An [`Activation`] — one per call or block entry, laid down in the frame's region: a pointer to
//!   the closure bindings, the builtin table's base pointer, the enclosing block's view, the
//!   callable it runs, and one slot per parameter and local. Its read half, the covariant
//!   [`ActivationView`], is what every reader takes; the activation adds only the door that binds.
//!
//! Every tier is generic over the callable a value may hold, the parameter [`crate::values::Value`]
//! takes — the activation and its view over the callable's family, since a slot holds its value
//! erased; `scope` threads it through and reads a callable only to resolve an edge capture.
//!
//! A read through a coordinate searches nothing by name. The walk `EVAL` runs is
//! [`BodyShape::for_eval`], which resolves each free name through [`ActivationView::coordinate_of`] and lands
//! where the coordinate would.
//!
//! **Imports.** This module may name `crate::memory`, `crate::parse`, `crate::symbols`,
//! `crate::type_lattice` and `crate::values`, and no scheduler type. From `type_lattice` it names
//! the operator-group vocabulary — [`DeclaredGroup`](crate::type_lattice::DeclaredGroup) and its
//! [`ReductionMode`](crate::type_lattice::ReductionMode) — so a signature's operator channel and a
//! body's held group are one record. `tests::boundary` reads the source to hold it there.
//!
//! See [scope/README.md](scope/README.md).

mod activation;
mod builtins;
mod channels;
mod closure;
mod groups;
mod shape;
mod signature;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use signature::pair_name;

pub use activation::{Activation, ActivationView};
pub use builtins::Builtins;
pub use closure::ClosureBindings;
pub use groups::{BuiltinGroup, GroupFrame, is_equal, is_equality, is_unequal};
pub use shape::{
    BodyShape, BuiltinIndex, CaptureSlot, CaptureSource, CaptureSpec, Component, ComponentIndex,
    Coordinate, Mention, MentionClass, Position, ShapeError, ShapeKind, Site, Slot, Target, Unit,
    UnitWork,
};

//! Koan's lexical environments: what a name means at the point it is read, in three tiers.
//!
//! - The [`Shape`] — one per body, built once into program storage — declares the body's value and
//!   type names, classifies every mention eager or deferred, delimits the components its bindings
//!   form, and resolves every name the body reads to a [`Coordinate`].
//! - [`ClosureBindings`] — one run per callable, born from the enclosing activation through the
//!   shape's capture layout: a value word per capture, or an edge into the callable's own knot.
//! - An [`Activation`] — one per call or block entry, laid down in the frame's region: a pointer to
//!   the closure bindings, the builtin table's base pointer, the enclosing block activation, the
//!   callable it runs, and one slot per parameter and local.
//!
//! Every tier is generic over the callable a value may hold, the parameter [`crate::values::Value`]
//! takes; `scope` threads it through and reads a callable only to resolve an edge capture.
//!
//! A read through a coordinate searches nothing by name. The walk `EVAL` runs is
//! [`Shape::for_eval`], which resolves each free name through [`Activation::coordinate_of`] and lands
//! where the coordinate would.
//!
//! **Imports.** This module may name `crate::memory`, `crate::parse`, `crate::type_lattice` and
//! `crate::values`, and no scheduler type; outside `#[cfg(test)]` it names no `type_lattice` item.
//! `tests::boundary` reads the source to hold it there.
//!
//! See [scope/README.md](scope/README.md).

mod activation;
mod builtins;
mod channels;
mod closure;
mod roles;
mod shape;
mod signature;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use roles::{BodyKind, Role, roles};
pub(crate) use signature::pair_name;

pub use activation::{Activation, Binding};
pub use builtins::Builtins;
pub use closure::{ClosureBindings, ClosureRefused};
pub use shape::{
    BuiltinIndex, CaptureSlot, CaptureSource, CaptureSpec, Component, ComponentIndex, Coordinate,
    Mention, MentionClass, Position, Shape, ShapeError, ShapeKind, Site, Slot, Target,
};

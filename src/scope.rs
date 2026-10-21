//! Koan's lexical environments: what a name means at the point it is read, in three tiers.
//!
//! - The [`BodyShape`] — one per body, built once into program storage — declares the body's value and
//!   type names and its definitions' registrations, classifies every mention eager or deferred,
//!   delimits the components its bindings form, resolves every name the body reads to a
//!   [`Coordinate`], and every keyworded use to a [`CandidateList`] of them.
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
//! A read through a coordinate searches nothing by name, `EVAL` included: a quote value's code is
//! shaped where the program loads, so every free name of it is a capture its `EVAL` fills.
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
mod facts;
mod groups;
mod header;
mod keyed;
mod records;
mod relay;
mod settled;
mod shape;
mod shapes;
mod signature;
mod spelling;
mod tree;
mod typed;
mod writes;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use shape::IMPLICIT;
pub(crate) use signature::pair_label;
pub(crate) use spelling::{PathSpelling, SpelledRoot, path_spelling};

pub use activation::{Activation, ActivationView};
pub use builtins::Builtins;
pub use closure::ClosureBindings;
pub use facts::{
    Address, Binding, Bound, FactKey, Frontier, OldBinding, Residual, Seam, SeamAt, Store, UnitId,
    Unknown, frontier_at, merge_into,
};
pub use groups::{BuiltinGroup, GroupFrame, is_equal, is_equality, is_unequal};
pub use header::{HalfHeader, RegionHeader};
pub use keyed::{Keyed, KeyedEntry};
pub use records::{
    Body, Born, Closing, Deps, Folds, LoadFacts, NamedVariable, SiteDeps, Typed, UnitFacts,
};
pub use relay::{ChainRelay, Relay};
pub use settled::{ChannelSettled, Resumption, Settled, StaticsSettled};
pub use shape::{
    Arm, BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureSlot, CaptureSource,
    CaptureSpec, Component, ComponentIndex, Coordinate, Lands, Listed, Mention, MentionClass,
    Offer, PathId, Position, QuotedPart, Ranking, Registration, Segment, ShapeError, ShapeKind,
    Site, Slot, SpelledPath, SurfacedHead, Target, TopLevel, TypeExpression, Unit, UnitIndex,
    UnitWork, ViewRefused, Which, each_part, in_body_of, part_of, selection_refused, source_of,
    spelled, view_refused,
};
pub use shapes::{BindSite, Shapes};
pub use tree::{Binder, KnotNode, Leaf, NodeCapture, Print, Tree, Unfolded, Within};
pub use typed::{
    BodyClass, Caching, Callable, Elaboration, ElaborationDisplay, FunctionGroupMap, HandedTo,
    Handoff, InstanceRead, Mint, Narrowing, ParameterBinding, PathEntry, PathIndex, PathRead,
    Paths, Registered, Resolution, Resolved, Root, ShapeFacts, ShapeGroupMap, Source, Static,
    StaticCallable, StaticRegistered, StaticSolution, StaticType, Statics, UseStatics, Variable,
    ViewSlots, solutions,
};
pub use writes::Writes;

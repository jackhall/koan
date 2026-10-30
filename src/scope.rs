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
mod composition;
mod facts;
mod filled;
mod groups;
mod header;
mod keyed;
mod memo;
mod record_types;
mod records;
mod relay;
mod settlement_facts;
mod shape;
mod shapes;
mod signature;
mod spelling;
mod stamp;
mod tree;
mod typed;
mod writes;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use records::RecordSource;
#[cfg(test)]
pub(crate) use records::{RIGID_AT_RUN, TERM_PROBES, rigid_at_run};
pub(crate) use shape::IMPLICIT;
pub(crate) use signature::{pair_label, traced_quote};
pub(crate) use spelling::{PathSpelling, SpelledRoot, path_spelling};

pub use activation::{Activation, ActivationView, Reading};
pub use builtins::Builtins;
pub use closure::ClosureBindings;
pub use composition::{ComposedSyntax, Composition, quote_paths};
pub use facts::{Address, Binding, Bound, Cut, Frontier, Store, UnitId, Unknown};
pub use filled::{FilledSites, filled_sites};
pub use groups::{BuiltinGroup, GroupFrame, is_equal, is_equality, is_unequal};
pub use header::{HalfHeader, RegionHeader};
pub use keyed::{Keyed, KeyedEntry};
pub use memo::{DecisionMemo, Entry, EntryFamily, EventSite, MemoFamily, MemoKey, extended};
pub use record_types::Named;
pub use records::{
    AtRun, BodyAt, BodyRef, Born, Closing, CodeSyntax, ComposedRoot, Deps, DepsGathering, Facts,
    FactsOver, Folds, Layers, LoadFacts, LoadRecords, NamedVariable, Retypes, SiteDeps, Typed,
    UnitFacts, Written, tree_deps,
};
pub use settlement_facts::{Activated, Settlement};
pub use shape::{
    Arm, BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureChain, CaptureSlot,
    CaptureSource, CaptureSpec, Component, ComponentIndex, Coordinate, Lands, Listed, Located,
    Mention, MentionClass, Offer, PathId, Position, QuotedPart, Ranking, Registration, Segment,
    ShapeError, ShapeKind, Site, Sites, Slot, SpelledPath, SurfacedHead, Target, TopLevel,
    TypeExpression, Unit, UnitHolds, UnitIndex, UnitWork, ViewRefused, Which, each_part, follow,
    in_body_of, part_of, selection_refused, source_of, spelled, view_refused,
};
pub use shapes::{BindSite, Code, Origin, Shapes};
pub use stamp::{Ask, Asked, Level, Sought, Stamp};
pub use tree::{Binder, Identity, KnotNode, Leaf, NodeCapture, Tree, Unfolded, Within};
pub use typed::{
    BodyClass, Caching, Callable, CandidateRoute, Elaboration, ElaborationDisplay, Elimination,
    FunctionGroupMap, HandedTo, Handoff, InstanceRead, Kept, Mint, Narrowing, ParameterBinding,
    PathEntry, Pick, Point, Points, Registered, Resolution, Resolved, Root, Route, ShapeGroupMap,
    Source, Static, StaticCallable, StaticInstance, StaticRegistered, StaticType, Table,
    UseStatics, Variable, ViewSlots,
};
pub use writes::{KeyedWrites, Writes};

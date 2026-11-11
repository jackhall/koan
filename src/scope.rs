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
mod admission;
mod build_layer;
mod builtins;
mod channels;
mod closed;
mod closure;
mod composition;
mod facts;
mod fault;
mod filled;
mod groups;
mod header;
mod keyed;
mod memo;
mod question;
mod record_types;
mod records;
mod relay;
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
pub(crate) use relay::{Relay, relaid_error};
pub(crate) use shape::IMPLICIT;
pub(crate) use signature::{pair_label, traced_quote};
pub(crate) use spelling::{PathSpelling, SpelledRoot, path_spelling, tag_projection};

pub use activation::{Activation, ActivationView, Reading};
pub use admission::{Admission, Condition, Keeping};
pub use build_layer::{BuildLayer, Witnessing};
pub use builtins::Builtins;
#[cfg(test)]
pub(crate) use closed::CLOSED_LAID;
pub use closed::{ClosedTypes, closed_under};
pub use closure::ClosureBindings;
pub use composition::{ComposedSyntax, Composition, quote_paths};
pub(crate) use facts::Flattened;
pub use facts::{
    Address, Binding, Bound, Cut, Frontier, Mark, Noted, OpenKind, OpenMark, Store, UnitId,
    Unknown, Wake, settle_marks,
};
pub use fault::{
    Bottom, CodeRefused, CodeRefusedDisplay, CoercionRefused, ComposeRefused, ErrorField, Fault,
    FaultDisplay, FaultKind, Field, Hop, Inner, Line, Link, RefusedConstruction, Trace, Untieable,
    UntieableDisplay, UsingRefused, refused_construction, ungrouped,
};
pub use filled::{FilledSites, filled_sites};
pub use groups::{BuiltinGroup, GroupFrame, is_equal, is_equality, is_unequal};
pub use header::{HalfHeader, RegionHeader};
pub use keyed::{Keyed, KeyedEntry};
pub use memo::{
    Decided, Decision, DecisionMemo, Entry, EntryFamily, EventSite, FANOUT, MemoFamily, MemoKey,
    RoomFamily, extended, spine_digest, witnessed_digest,
};
#[cfg(test)]
pub(crate) use memo::{HASH_MASK, PROBED};
pub use question::{
    Answer, Answered, Levels, Question, answer, free_variables, keeps, rebound, unsatisfiable,
};
pub use record_types::{CloseLaying, Closed, Closer, Laying, Named};
pub use records::{
    AtRun, BodyAt, BodyRef, Born, CallTrace, Closing, CodeSyntax, ComposedRoot, Content, Deps,
    DepsGathering, Facts, FactsOver, Fixed, Folds, Held, Layers, LoadFacts, LoadLayer, LoadRecords,
    NamedVariable, Open, Restamp, Retypes, SiteDeps, SlotMap, Traced, Typed, UnitFacts, Untraced,
    Written, tree_deps,
};
pub use shape::{
    Arm, BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureChain, CaptureSlot,
    CaptureSource, CaptureSpec, Component, ComponentIndex, Coordinate, Guard, Lands, Listed,
    Located, Mention, MentionClass, Offer, PathId, Position, QuotedPart, Ranking, RefusedLine,
    Registration, Repeated, Scalar, Segment, ShapeError, ShapeKind, Shown, Site, Sites, Slot, Span,
    SpelledPath, SurfacedHead, Target, TopLevel, TypeExpression, Unit, UnitHolds, UnitIndex,
    UnitWork, ViewRefused, Which, arm_refused, each_part, follow, in_body_of, part_of,
    selection_refused, source_of, spelled, view_refused,
};
pub use shapes::{BindSite, CallSite, Code, Origin, Shapes};
pub use stamp::{Ask, Asked, Level, SiteRequest, Sought, Stamp};
pub use tree::{
    Bare, Binder, Identity, KnotNode, Leaf, NodeCapture, Revealed, Sealing, Tree, Unfolded,
    ViewRecord, Within, hides,
};
pub use typed::{
    Arms, BodyClass, Caching, Callable, CandidateRoute, Elaboration, ElaborationDisplay,
    Elimination, FunctionGroupMap, HandedTo, Handoff, InstanceRead, Kept, Mint, Narrowing,
    ParameterBinding, PathEntry, PathName, Pick, Point, Points, Registered, Resolution, Resolved,
    Root, Route, ShapeGroupMap, Static, StaticCallable, StaticInstance, StaticRegistered,
    StaticType, Table, UseStatics, Variable, ViewSlots,
};
pub use writes::{KeyedWrites, Writes};

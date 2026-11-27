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
mod application;
mod build_layer;
mod builtins;
mod channels;
mod closure;
mod composition;
mod derived;
mod facts;
mod fault;
mod filled;
mod groups;
mod header;
mod heads;
mod keyed;
mod nominal;
mod question;
mod record_types;
mod records;
mod relay;
mod rigid;
mod rule;
mod runs;
mod shape;
mod shapes;
mod signature;
mod signed;
mod site;
mod spelling;
mod tree;
mod typed;
mod writes;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) use relay::relaid_error;
pub(crate) use shape::IMPLICIT;
pub(crate) use signature::{pair_label, traced_quote};
pub(crate) use spelling::{PathSpelling, SpelledRoot, path_spelling, tag_projection};

pub use crate::type_lattice::DerivedId;
pub use activation::{Activation, ActivationView, Reading};
pub use build_layer::BuildLayer;
pub use builtins::Builtins;
pub use closure::{ClosureBindings, Encloser};
pub use composition::{ComposedSyntax, Composition, quote_paths};
pub use derived::{
    Binds, Carrier, Decider, Derived, DerivedName, DerivedTerm, LevelsOnly, NameAddress, Spellings,
    handed,
};
pub(crate) use facts::Flattened;
pub use facts::{
    Address, Binding, Bound, Cut, Frontier, Mark, Noted, OpenKind, OpenMark, Store, UnitId,
    Unknown, settle_marks,
};
pub use fault::{
    Bottom, CodeRefused, CodeRefusedDisplay, CoercionRefused, ComposeRefused, ErrorField, Fault,
    FaultDisplay, FaultKind, Field, Hop, Inner, Line, Link, RefusedConstruction, Trace, Untieable,
    UntieableDisplay, UsingRefused, refused_construction, ungrouped,
};
pub use filled::{FilledSites, filled_sites};
pub use groups::{BuiltinGroup, GroupFrame, is_equal, is_equality, is_unequal};
pub use header::{HalfHeader, RegionHeader};
pub use heads::{
    applied, applied_union, closed_variant, constructed_upper, declared_field, declared_member,
    declared_variant, head_params, judge_signed, needing, pinned, shown_view, signature_outside,
    tag_union, unbuilt, variant_label, variant_named, variants, view_signature,
};
pub use keyed::{Keyed, KeyedEntry};
pub use nominal::{Handed, Nominals, Schema, Showing, sealed_nominals};
pub use question::{
    Answer, Levels, LevelsOver, Posed, Question, StoreBinds, answer, free_variables, is_module,
    rebound, unsatisfiable,
};
pub use record_types::{CloseLaying, Closed, Closer, Laying, Named, rebuilt_closed};
pub use records::{
    AtRun, BodyAt, BodyRef, Born, CaptureRead, Closing, CodeSyntax, ComposedRoot, Content, Facts,
    FactsOver, Fixed, Folds, Held, Holder, Layers, LoadFacts, LoadRecords, NamedVariable, Restamp,
    Retypes, Traced, Typed, UnitFacts, UnitRecord, UseRead, Written, reads_module_binders,
};
#[cfg(test)]
pub(crate) use records::{REBOUND, RIGID_AT_RUN, RecordSource, TERM_PROBES, rigid_at_run};
pub use rigid::{
    apart, bare_variable, is_variable, kind_varies, lower_end_outside_signed, may_be_never,
    rigid_answer, rigid_verdict, rigid_within, signature_name, variable_outside,
};
pub use rule::Rule;
pub use runs::{GuardKind, Run, first_ranked, guard_outranks, never_runs, solve_read, winner};
pub use shape::{
    Arm, BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureChain, CaptureSlot,
    CaptureSource, CaptureSpec, Component, ComponentIndex, Coordinate, Guard, Lands, Listed,
    ListedMember, Located, Mention, MentionClass, NeverSatisfied, Offer, PathId, Position,
    QuotedPart, Ranking, Registration, Repeated, Scalar, Segment, ShapeError, ShapeKind, Shown,
    Site, Sites, Slot, SpelledPath, SurfacedHead, Target, TopLevel, TypeExpression, Unit,
    UnitHolds, UnitIndex, UnitWork, ViewRefused, Which, arm_refused, each_part, follow, in_body_of,
    part_of, refused_lines, selection_refused, shown_against, source_of, spelled, view_refused,
};
pub use shapes::{BindSite, CallSite, Code, Origin, Shapes};
pub use signed::{Signed, Signing, sealed_signature};
pub use site::{
    ArmChoice, ArmsLayout, AskedBy, Asks, EncloserWords, GuardAsks, Handles, InstanceCheck, Lack,
    Need, Passes, Passing, Payload, PointAsk, Rebinds, Routing, Selectable, Selecting, Selection,
    SiteAnswer, SiteAnswers, SiteQuestion, SiteRule, decide,
};
pub use tree::{
    Bare, Binder, BinderAt, Called, Callee, Identity, KnotNode, Leaf, NodeCapture, Revealed,
    Sealing, Tree, Unfolded, ViewRecord, Within, hides,
};
pub use typed::{
    Arms, AsWritten, BodyClass, Caching, Callable, CandidateRoute, Elaboration, ElaborationDisplay,
    Elimination, FunctionGroupMap, HandedTo, Handoff, Holding, InstanceRead, Kept, MemberWanted,
    Narrowing, ParameterBinding, PathEntry, PathName, Pick, Point, Points, Rebounds, Registered,
    Resolution, Resolved, Root, Route, ShapeGroupMap, Shaped, Spelling, Static, StaticCallable,
    StaticInstance, StaticRegistered, StaticType, Table, Unresolved, Unwalked, UseStatics,
    Variable, ViewSlots, WIDEST, Width, written_or,
};
pub use writes::Writes;

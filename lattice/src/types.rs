//! The type lattice: a closed algebra over interned type nodes.
//!
//! The node vocabulary, the tiers types are interned in, the identity recipe, the structural
//! relations between types, and the unifier that solves a quantified position. Nothing else. It
//! imports the classified symbol types from [`symbols`](crate::symbols), with the bump tier from
//! [`bump`](crate::bump). No value, cell, AST, scope, working part or execute-side type reaches it,
//! since the crate depends on no koan code — the parser included, which is what keeps the two from
//! naming each other. Everything that matches a type against something that is *not* a type — a
//! value, a parser part, a declaration — lives with that thing and calls in here.
//!
//! # The relations
//!
//! [`is_subtype_of`] is one reflexive partial order over concrete types ([`KType`]), memoized
//! through the tiers' verdict tables, which never solves; every construction reads it. [`fits`]
//! contains it and solves — a scheme fits another through an instance, a module's signature fits a
//! declared one through its members — and every question reads it, [`satisfied_by`] from a slot's
//! side. [`join`] is the least upper bound — the larger operand when the two are ordered, their
//! canonical union otherwise — and [`meet`] the greatest lower bound. [`admits_with`] walks a
//! declared type against a carried one and collects what would solve the quantified positions;
//! [`Collector::solve`] bounds each variable by a pair of ends and binds its least instance.
//! [`sig_fits`] is *fits* over two signature types: each a set of applications of declared
//! signatures, which meet at the union of their sets — two unordered signature types join to their
//! union — and [`shape_specificity`] ranks two candidates under one bucket key, class by class
//! through a shape's priority classes: [`admit_by_class`] is what a keyworded call admits by, and
//! [`select_by_class`] the elimination a candidate list runs.
//!
//! # Storage
//!
//! Types live in tiers ([`tiers`]): the [`Frozen`] load the load builds, the [`Agent`] tier one
//! agent rests above it, and a [`Branch`] per habitat half the run builds in, laid in that half
//! and released with it. A [`TypeChain`] reads the frozen load, the agent tier, a scratch
//! habitat's region tier and its own half's tier, and interns into its own;
//! every node it lays — with every slice a node holds — lives in that half, as do its verdicts.
//! Nothing the lattice owns carries drop glue, so a half releases its tier whole. A crossing makes
//! another half's handles resolve in a chain by a link, a merge or an adoption ([`transfer`]).
//! Every door and relation that needs a transient buffer takes a scratch allocator from its caller
//! and builds the buffer there, so interning a type or running a relation touches the global heap
//! nowhere.
//!
//! # Writing a new walk
//!
//! Every structural recursion here goes through one of the two drivers in [`walk`], with rendering
//! the single hand-written exhaustive match. [`walk`]'s own module documentation says which driver
//! a new walk wants and what it must supply; adding a compound node variant is a compile error at
//! the drivers' arm tables, at the descent-knob sites, in [`render`], and at the stored node's
//! three conversion tables, and nowhere else.
//!
//! # Laws, not shapes
//!
//! The lattice is tested by its laws, as properties over generated type trees interned into a live
//! chain over two tiers ([`tests::properties`]). Hand-written tests remain only where a law cannot
//! express the shape, and each says which.
//!
//! See [types/README.md](types/README.md).

mod chain;
mod component;
mod digest;
mod handle;
mod kind;
mod lattice;
mod node;
mod operators;
mod order;
mod ranking;
mod record;
mod render;
mod run;
mod schema;
mod shape;
mod sig_relations;
mod signatures;
mod stored;
mod substitute;
mod term;
mod tiers;
mod transfer;
mod typed;
mod unify;
mod verdicts;
mod walk;
mod window;

#[cfg(test)]
mod tests;

pub use chain::{GroupIntern, TypeChain};
pub use digest::TypeDigest;
pub use handle::{
    DeclaredType, DeclaredView, KType, Parametric, Scheme, TypeHandle, builtin_types,
};
pub use kind::KKind;
pub use node::{ContentKey, Group, NodeSchema, TypeNode, Variable};
pub use operators::{FoldDirection, ReductionMode};
pub use ranking::{Judged, Verdict};
pub use record::Record;
pub use render::{
    TypeNameDisplay, display_name, display_symbol, render_fits_failure, render_keyworded_head,
    render_symbol,
};
pub use run::{Elements, Run};
pub use schema::{
    DeclaredGroup, Members, SchemaDraft, SigOrigin, SigSchema, constructor_param_names, member,
};
pub use shape::{
    Classes, DeferredReturnSurface, DispatchTokenElement, RawRank, Specificity, dense_classes,
    scheme_return, scheme_slots, shape_return, shape_slots,
};
pub use sig_relations::{FitsFailure, InstanceFailure};
pub use substitute::Side;
pub use term::Term;
pub use tiers::{Agent, Branch, Frozen, Habitat, Halves, Own, RegionRead, RootBuilder};
#[cfg(any(test, feature = "counters"))]
pub use transfer::{ADOPT_ASKED, ADOPT_HITS, ADOPT_LAID, ADOPT_PROBES};
pub use typed::{
    Substitutable, admit_by_class, bound_above, erase_rigid, fits, fits_application,
    instance_under, instantiate_quantified, is_subtype_of, join, join_iter, judge_by_class,
    lower_end_outside, meet, outranks, read_through, satisfied_by, scheme_bound_above,
    select_by_class, shape_specificity, sig_fits, solving_slots, substitute_levels,
    substitute_parameters, substitute_quantified, tied_members,
};
pub use unify::{Collector, Interval, UnifyFailure, admits_with, intervals};
pub use walk::Variance;
pub use window::{RecursiveGroupWindow, RelativeSchema, SealedGroup};

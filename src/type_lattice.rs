//! The type lattice: a closed algebra over interned type nodes.
//!
//! The node vocabulary, the interning registry, the identity recipe, the structural relations
//! between types, and the unifier that solves a quantified position. Nothing else. It imports the
//! classified symbol types from [`symbols`](crate::symbols) and
//! [`ScopeId`](crate::memory::ScopeId), and no value, cell, AST, scope, working part or
//! execute-side type reaches it — the parser included, which is what keeps the two from naming
//! each other. Everything that
//! matches a type against something that is *not* a type — a value, a parser part, a declaration —
//! lives with that thing and calls in here.
//!
//! [`tests::boundary`] walks this module's own source and fails on any other `crate::` path.
//!
//! # The relations
//!
//! [`is_subtype_of`] is one reflexive partial order, memoized through the registry's verdict edges,
//! which never solves; every construction reads it. [`fits`] contains it and solves — a quantified
//! binder fits another through an instance, a module's signature fits a declared one through its
//! members — and every question reads it, [`satisfied_by`] from a slot's side. [`join`] is the least upper bound — the larger operand when the two are ordered,
//! their canonical union otherwise — and [`meet`] the greatest lower bound. [`admits_with`] walks a
//! declared type against a carried one and collects what would solve the quantified positions;
//! [`Collector::solve`] bounds each variable by a pair of ends and binds its least instance.
//! [`sig_fits`] is *fits* over two signature types: each a set of applications of declared
//! signatures, which meet at the union of their sets — two unordered signature types join to their
//! union — and [`shape_specificity`] ranks two candidates
//! under one bucket key, class by class through a shape's priority classes: [`admit_by_class`] is
//! what a keyworded call admits by, and [`select_by_class`] the elimination a candidate list runs.
//!
//! # Storage
//!
//! A [`TypeRegistry`] is built over the run region's bump allocator, and every node it interns —
//! with every slice a node holds — lives in that region, as does the verdict table, a fixed cache
//! laid there once: nothing the lattice owns carries drop glue, and the region releases it whole.
//! Every door and relation that needs a transient buffer takes a scratch allocator from its caller
//! and builds the buffer there, so interning a type or running a relation touches the global heap
//! nowhere.
//!
//! # Writing a new walk
//!
//! Every structural recursion here goes through one of the two drivers in [`walk`], with rendering
//! the single hand-written exhaustive match. [`walk`]'s own module documentation says which driver
//! a new walk wants and what it must supply; adding a compound node variant is a compile error at
//! the drivers' arm tables, at the descent-knob sites, and in [`render`], and nowhere else.
//!
//! # Laws, not shapes
//!
//! The lattice is tested by its laws, as properties over generated type trees interned into a live
//! registry ([`tests::properties`]). Hand-written tests remain only where a law cannot express the
//! shape, and each says which.
//!
//! See [type_lattice/README.md](type_lattice/README.md).

mod digest;
mod handle;
mod kind;
mod lattice;
mod node;
mod operators;
mod order;
mod ranking;
mod record;
mod registry;
mod render;
mod run;
mod schema;
mod shape;
mod sig_relations;
mod signatures;
mod substitute;
mod typed;
mod unify;
mod walk;
mod window;

#[cfg(test)]
mod tests;

pub use digest::TypeDigest;
pub use handle::{DeclaredType, Handle, KType, Parametric, Scheme, TypeHandle, builtin_types};
pub use kind::KKind;
pub use node::{NodeSchema, TypeNode};
pub use operators::{FoldDirection, ReductionMode};
pub use ranking::{Judged, Verdict};
pub use record::Record;
pub use registry::{GroupIntern, TypeRegistry};
pub use render::{
    TypeNameDisplay, display_name, display_symbol, render_fits_failure, render_keyworded_head,
    render_symbol,
};
pub use run::{Elements, Run};
pub use schema::{
    DeclaredGroup, Members, SchemaDraft, SigOrigin, SigSchema, constructor_param_names, is_shape,
    member, scheme_return, scheme_slots, shape_keys_equal, shape_return, shape_slots,
};
pub use shape::{DeferredReturnSurface, DispatchTokenElement, RawRank, Specificity, dense_classes};
pub use sig_relations::FitsFailure;
pub use substitute::{Side, Variable};
pub use typed::{
    Substitutable, admit_by_class, bound_above, class_at_least, erase_rigid, fits,
    fits_application, instantiate_quantified, is_subtype_of, join, join_iter, judge_by_class, meet,
    quantifier_bounds, read_through, satisfied_by, scheme_bound_above, select_by_class,
    shape_specificity, sig_fits, substitute_levels, substitute_parameters, substitute_quantified,
};
pub use unify::{Collector, Interval, UnifyFailure, admits_with, intervals};
pub use walk::Variance;
pub use window::{RecursiveGroupWindow, RelativeSchema, SealedGroup};

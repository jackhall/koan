//! The type lattice: a closed algebra over interned type nodes.
//!
//! The node vocabulary, the interning registry, the identity recipe, the structural relations
//! between types, and the unifier that solves a quantified position. Nothing else. It imports the
//! label and symbol types from [`parse`](crate::parse) and [`ScopeId`](crate::memory::ScopeId), and
//! no value, cell, AST, scope, working part or execute-side type reaches it. Everything that
//! matches a type against something that is *not* a type — a value, a parser part, a declaration —
//! lives with that thing and calls in here.
//!
//! [`tests::boundary`] walks this module's own source and fails on any other `crate::` path.
//!
//! # The relations
//!
//! [`is_subtype_of`] is one reflexive partial order, memoized through the registry's verdict edges;
//! [`is_more_specific_than`] is its strict version and [`satisfied_by`] the same question read from
//! a slot's side. [`join`] is the least upper bound — the larger operand when the two are ordered,
//! their canonical union otherwise — and [`meet`] the greatest lower bound. [`admits_with`] walks a
//! declared type against a carried one and collects what would solve the quantified positions;
//! [`Collector::solve`] takes a maximum, a minimum, or the bound, and never mints a union nobody
//! wrote. [`sig_subtype`] and [`meet_schemas`] are the order and the meet over two signature schemas —
//! two unordered signatures join to their union — and [`shape_specificity`] ranks two candidates under one bucket key.
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
//! See [design/typing/type-lattice.md](../design/typing/type-lattice.md).

mod digest;
mod handle;
mod kind;
mod lattice;
mod node;
mod operators;
mod order;
mod record;
mod registry;
mod render;
mod schema;
mod shape;
mod sig_relations;
mod substitute;
mod unify;
mod walk;
mod window;

#[cfg(test)]
mod tests;

pub use digest::TypeDigest;
pub use handle::{KType, builtin_types};
pub use kind::KKind;
pub use lattice::{join, join_iter, meet};
pub use node::{NodeSchema, TypeNode};
pub use operators::{FoldDirection, ReductionMode};
pub use order::{is_more_specific_than, is_subtype_of, satisfied_by};
pub use record::Record;
pub use registry::{ShapeIntern, TypeRegistry};
pub use render::{
    TypeNameDisplay, display_label, display_name, render_keyworded_head, render_label,
    render_sig_failure,
};
pub use schema::{
    DeclaredGroup, OperatorMembers, SigSchema, TypeMemberMap, canonical_groups,
    canonical_overloads, constructor_param_names, is_abstract_sig_member, is_shape,
    shape_keys_equal, shape_return, shape_slots,
};
pub use shape::{DeferredReturnSurface, DispatchTokenElement, Specificity};
pub use sig_relations::{
    SigSubtypeFailure, most_specific_ktype, select_keyworded_satisfier, shape_specificity,
    sig_subtype,
};
pub use substitute::{
    canonicalize_binder, erase_quantified, erase_rigid, instantiate_quantified, quantifier_bounds,
    slot_more_specific_or_equal, slot_satisfied_by, slot_types_equal, substitute_quantified,
    substitute_sig_members,
};
pub use unify::{Collector, UnifyFailure, admits_with};
pub use walk::Variance;
pub use window::{RecursiveGroupWindow, RelativeSchema, SealedGroup};

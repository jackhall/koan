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
//! wrote. [`sig_subtype`], [`join_schemas`] and [`meet_schemas`] are the same three questions over
//! two signature schemas, and [`shape_specificity`] ranks two candidates under one bucket key.
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

pub mod digest;
pub mod handle;
pub mod kind;
pub mod lattice;
pub mod node;
pub mod operators;
pub mod order;
pub mod record;
pub mod registry;
pub mod render;
pub mod schema;
pub mod shape;
pub mod sig_relations;
pub mod substitute;
pub mod unify;
pub mod walk;
pub mod window;

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
pub use registry::{Relation, ShapeIntern, TypeRegistry};
pub use render::{
    display_label, display_name, name, name_under, render_declared_group, render_keyworded_head,
    render_label, render_sig_failure, surface_opens_sigil, write_name, write_shape_surface,
};
pub use schema::{
    DeclaredGroup, OperatorMembers, SigSchema, TypeMemberMap, canonical_groups,
    canonical_overloads, constructor_param_names, is_abstract_sig_member, is_shape,
    name_sets_equal, shape_keys_equal, shape_quantifiers, shape_return, shape_slots,
};
pub use shape::{DeferredReturnSurface, DispatchTokenElement, Specificity};
pub use sig_relations::{
    SigSubtypeFailure, admits_slots, join_schemas, meet_schemas, most_specific_ktype,
    select_keyworded_satisfier, shape_specificity, sig_subtype,
};
pub use substitute::{
    abstract_references, canonicalize_binder, collect_siblings, erase_quantified, erase_rigid,
    instantiate_quantified, quantifier_bounds, rewrite_siblings, slot_more_specific_or_equal,
    slot_satisfied_by, slot_types_equal, substitute_quantified, substitute_sig_members,
};
pub use unify::{Collector, UnifyFailure, admits_with};
pub use walk::Variance;
pub use window::{
    PendingMember, RecursiveGroupWindow, RelativeSchema, SealBinderInput, SealMemberInput,
    SealedGroup, seal_group,
};

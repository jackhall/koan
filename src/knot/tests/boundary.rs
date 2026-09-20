//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `knot` is the top of the rewrite's value stack: it may name `elaborate`, `memory`, `parse`,
//! `scope`, `symbols`, `type_lattice` and `values`, and nothing else in the crate — no scheduler,
//! no builtins. Outside its tests it holds no owning heap type.
//!
//! The direction *inside* `knot` — [`function`](crate::knot::function) and
//! [`module`](crate::knot::module) reach their vocabulary through the facade and never each other —
//! is spelled in `super`-relative paths, which this scanner does not read. It is held by review.

/// The path prefixes `knot` may name, beside the scanner that reads them and the scope plans its
/// law runs.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::knot",
    "crate::memory",
    "crate::parse",
    "crate::scope",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn knot_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("knot", PREFIXES, &[]);
}

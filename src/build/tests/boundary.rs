//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `build` may name `memory`, `scope` and `values` and nothing else in the crate; outside its tests it holds
//! no owning heap type.

/// The path prefixes `build` may name, beside the scanner that reads them.
const PREFIXES: &[&str] = &[
    "crate::build",
    "crate::memory",
    "crate::scope",
    "crate::tests::boundary",
    "crate::values",
];

#[test]
fn build_names_only_scope_and_values_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("build", PREFIXES, &[]);
}

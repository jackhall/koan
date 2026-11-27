//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `build` may name `memory`, `scope`, `type_lattice` and `values`, and `SourceRef` (where a call
//! is written), and nothing else in the crate; outside its tests it holds no owning heap type
//! but the one the binding graph's `Facts` trait hands back: a term's mentions.

/// The path prefixes `build` may name, beside the scanner that reads them.
const PREFIXES: &[&str] = &[
    "crate::build",
    "crate::memory",
    "crate::scope",
    "crate::source::SourceRef",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

/// The one owning type outside the tests: the mentions the crate's `Facts::mentions` returns.
const OWNING_ALLOWED: &[(&str, &str)] = &[("src/build/lay.rs", "-> Vec<(NameId, Nesting)>")];

#[test]
fn build_names_only_scope_and_values_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("build", PREFIXES, OWNING_ALLOWED);
}

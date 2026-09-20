//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `values` may name `memory`, `parse`, `source`, `symbols` and `type_lattice` and nothing else in
//! the crate;
//! outside its tests it holds no owning heap type, so everything it builds rests in a region.

/// The path prefixes `values` may name, beside the scanner that reads them.
const PREFIXES: &[&str] = &[
    "crate::memory",
    "crate::parse",
    "crate::source",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn values_names_only_its_four_modules_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("values", PREFIXES, &[]);
}

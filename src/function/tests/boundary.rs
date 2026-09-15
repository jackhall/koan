//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `function` may name `elaborate`, `memory`, `parse`, `scope`, `type_lattice` and `values` and
//! nothing else in the crate — no scheduler; outside its tests it holds no owning heap type.

/// The path prefixes `function` may name, beside the scanner that reads them and the scope plans
/// its law runs.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::function",
    "crate::memory",
    "crate::parse",
    "crate::scope",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn function_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("function", PREFIXES, &[]);
}

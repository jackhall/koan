//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `module` is the top of the rewrite's value stack: it may name `elaborate`, `function`,
//! `memory`, `parse`, `scope`, `type_lattice` and `values`, and nothing else in the crate — no
//! scheduler, no builtins. Outside its tests it holds no owning heap type.

/// The path prefixes `module` may name, beside the scanner that reads them and the scope plans its
/// law runs.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::function",
    "crate::memory",
    "crate::module",
    "crate::parse",
    "crate::scope",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn module_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("module", PREFIXES, &[]);
}

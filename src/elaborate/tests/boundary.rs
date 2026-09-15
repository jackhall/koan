//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `elaborate` may name `memory`, `parse`, `scope`, `type_lattice` and `values` and nothing else in
//! the crate — no function layer and no scheduler; outside its tests it holds no owning heap type.

/// The path prefixes `elaborate` may name, beside the macro that mints a static name and the
/// scanner that reads them.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::memory",
    "crate::parse",
    "crate::scope",
    "crate::static_name",
    "crate::tests::boundary",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn elaborate_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("elaborate", PREFIXES, &[]);
}

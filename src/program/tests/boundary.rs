//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `program` may name the layers it stands over — `elaborate`, `knot`, `memory`, `parse`,
//! `scheduler`, `scope`, `symbols`, `type_lattice` and `values`. It holds no owning heap type
//! outside its tests.

/// The path prefixes `program` may name.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::knot",
    "crate::memory",
    "crate::parse",
    "crate::program",
    "crate::scheduler",
    "crate::scope",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn program_names_only_the_layers_below_it_and_holds_no_heap() {
    crate::tests::boundary::holds("program", PREFIXES, &[]);
}

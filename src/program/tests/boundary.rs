//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `program` may name the layers it stands over — `elaborate`, `knot`, `memory`, `parse`,
//! `scheduler`, `scope`, `symbols`, `type_lattice` and `values`. Outside its tests it holds no
//! owning heap type but a refused load's rendered shape error, reported once and never stored.

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

/// The one owning type outside the tests: a refused load's rendered shape error.
const OWNING_ALLOWED: &[(&str, &str)] = &[("src/program/record.rs", "rendered: String")];

#[test]
fn program_names_only_the_layers_below_it_and_holds_no_heap() {
    crate::tests::boundary::holds("program", PREFIXES, OWNING_ALLOWED);
}

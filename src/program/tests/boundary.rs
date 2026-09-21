//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `program` may name the layers it stands over — `memory`, `parse`, `scheduler`, `symbols` and
//! `type_lattice` — and its tests may name `knot` for the values their steps carry. It holds no
//! owning heap type outside its tests.

/// The path prefixes `program` may name.
const PREFIXES: &[&str] = &[
    "crate::knot",
    "crate::memory",
    "crate::parse",
    "crate::program",
    "crate::scheduler",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::type_lattice",
];

#[test]
fn program_names_only_the_layers_below_it_and_holds_no_heap() {
    crate::tests::boundary::holds("program", PREFIXES, &[]);
}

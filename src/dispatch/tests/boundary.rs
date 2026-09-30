//! The import boundary, as a test over this module's own source.
//!
//! `dispatch` stands over the program and every layer below it — `elaborate`, `knot`, `memory`,
//! `parse`, `program`, `scheduler`, `scope`, `symbols`, `type_lattice` and `values` — and nothing
//! names it but the embedder.

/// The path prefixes `dispatch` may name.
const PREFIXES: &[&str] = &[
    "crate::dispatch",
    "crate::elaborate",
    "crate::knot",
    "crate::memory",
    "crate::parse",
    "crate::program",
    "crate::scheduler",
    "crate::scope",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn dispatch_names_only_the_layers_below_it() {
    crate::tests::boundary::holds("dispatch", PREFIXES, &[]);
}

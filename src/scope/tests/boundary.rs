//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `scope` may name `memory`, `parse`, `symbols`, `type_lattice` and `values` and nothing else in
//! the crate —
//! no scheduler type; outside its tests it holds no owning heap type but the one error that
//! carries a list of names, so everything it builds rests in program storage or a region; and it
//! spells no lifetime `cellgraph` and `memory` retired. The compiler checks none of that, so this
//! test reads the files.

/// The path prefixes `scope` may name, beside the macro that mints a static name.
const PREFIXES: &[&str] = &[
    "crate::memory",
    "crate::parse",
    "crate::scope",
    "crate::static_name",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

/// The one owning type outside the tests: an eager cycle's member names, reported once and never
/// stored.
const OWNING_ALLOWED: &[(&str, &str)] = &[(
    "src/scope/shape.rs",
    "EagerCycle { members: Vec<BinderSymbol> }",
)];

#[test]
fn scope_names_only_its_four_modules_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("scope", PREFIXES, OWNING_ALLOWED);
}

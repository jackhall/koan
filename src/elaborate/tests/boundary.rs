//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `elaborate` may name `memory`, `parse`, `scope`, `source`, `symbols`, `type_lattice` and `values` and
//! nothing else in the crate — no function layer and no scheduler; outside its tests it holds no owning heap type.

/// The path prefixes `elaborate` may name, beside the macro that mints a static name and the
/// scanner that reads them.
const PREFIXES: &[&str] = &[
    "crate::elaborate",
    "crate::memory",
    "crate::parse",
    "crate::scope",
    "crate::source",
    "crate::static_name",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn elaborate_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("elaborate", PREFIXES, &[]);
}

/// The fact accessors and write doors on a shape that the type half never calls: every fact it
/// reads comes through the [`TypeContext`](crate::elaborate::TypeContext) the walk hands it.
const ACCESSORS: &[&str] = &[
    "declared_type",
    "registered_type",
    "callable_type",
    "typed_expression",
    "type_expressions",
    "in_body",
];

#[test]
fn the_type_half_reads_every_fact_through_its_context() {
    use crate::tests::boundary::{mode_branches, store_bypasses, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("src/elaborate/unit.rs");
    let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
    // The scan bites on a read off the shape, and passes a read through the context.
    let reads = "shape.declared_type(slot)\ncontext.declared(binder)";
    assert_eq!(store_bypasses(reads, ACCESSORS, &[]).len(), 1);
    let mut offenders = store_bypasses(&code, ACCESSORS, &[]);
    offenders.extend(mode_branches(&code));
    assert!(
        offenders.is_empty(),
        "read past the context:\n{}",
        offenders.join("\n")
    );
}

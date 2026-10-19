//! The import boundary, as a test over this module's own source.
//!
//! `dispatch` stands over the program and every layer below it — `elaborate`, `knot`, `memory`,
//! `parse`, `program`, `scheduler`, `scope`, `source`, `symbols`, `type_lattice` and `values` — and
//! nothing names it but the embedder.

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
    "crate::source",
    "crate::symbols",
    "crate::tests::allocation_count",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

#[test]
fn dispatch_names_only_the_layers_below_it() {
    crate::tests::boundary::holds("dispatch", PREFIXES, &[]);
}

/// The cell accessors and write doors the static pass reaches only through its store.
const ACCESSORS: &[&str] = &[
    "declared_type",
    "registered_type",
    "callable_type",
    "in_body",
    "resolution",
    "statics",
    "born_instance",
    "group_levels",
    "typed_expression",
    "type_expressions",
    "declared_variables",
    "type_captures",
    "path_reads",
    "path_read",
    "path_entry",
    "paths",
    "through_at",
    "binder_type",
    "value_type",
    "refusal",
    "shape_facts",
    "channel_residual",
    "statics_residual",
    "fix_statics",
    "fix_born_instance",
    "fix_type_captures",
    "fix_shape_facts",
    "fix_resolutions",
    "path_closed",
    "fix_paths",
    "fix_caching",
    "fix_statics_residual",
    "refuse_typing",
];

/// The fields of a level the static pass reads and writes only through its store.
const FIELDS: &[&str] = &[
    "binders",
    "statements",
    "parts",
    "uses",
    "selections",
    "settled",
    "instances",
    "named",
    "throughs",
    "trees",
];

#[test]
fn the_static_pass_reaches_every_fact_through_its_store() {
    use crate::tests::boundary::{collect_sources, store_bypasses, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pass = manifest.join("src/dispatch/statics");
    let mut sources = vec![manifest.join("src/dispatch/statics.rs")];
    collect_sources(&pass, &mut sources);
    let mut offenders = Vec::new();
    for source in sources
        .iter()
        .filter(|source| !source.ends_with("store.rs"))
    {
        let code = strip_comments_and_strings(&std::fs::read_to_string(source).unwrap());
        for found in store_bypasses(&code, ACCESSORS, FIELDS) {
            offenders.push(format!("{}:{found}", source.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "read past the store:\n{}",
        offenders.join("\n")
    );
}

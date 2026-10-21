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
    // The store's two halves and the entries that build them; every other file holds rules.
    let rules = |source: &&std::path::PathBuf| {
        !["store.rs", "resume.rs", "entry.rs"]
            .iter()
            .any(|half| source.ends_with(half))
    };
    for source in sources.iter().filter(rules) {
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

/// The run's readers of the load's facts: each reads them through `LoadFacts` alone.
const READERS: &[&str] = &[
    "src/dispatch/evaluate.rs",
    "src/dispatch/builtins.rs",
    "src/program/body.rs",
    "src/program/specialize.rs",
    "src/knot.rs",
    "src/knot/function.rs",
    "src/knot/code.rs",
    "src/knot/tie.rs",
    "src/knot/module.rs",
    "src/knot/module/birth.rs",
    "src/knot/module/view.rs",
    "src/elaborate/reads.rs",
    "src/elaborate/paths.rs",
    "src/scope/typed.rs",
    "src/scope/activation.rs",
    "src/scope/closure.rs",
    "src/scope/tree.rs",
];

/// The cell accessors a run's reader might call on a shape, past the static pass's own: what
/// `LoadFacts` answers in their place, and `shape_at`, which the shape table answers.
const READ_ACCESSORS: &[&str] = &[
    "caches",
    "caching",
    "capture_count",
    "class",
    "digested_count",
    "digested_index",
    "handoffs",
    "instance_at",
    "mint_at",
    "mints_at",
    "named_contributions",
    "settled",
    "shape_at",
    "specialization_keys",
    "statement_type",
    "use_statics",
    "view_slots",
    "view_tree",
    "warm_at",
];

#[test]
fn the_run_reads_every_load_fact_through_the_read_door() {
    use crate::tests::boundary::{cell_reads, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let accessors: Vec<&str> = ACCESSORS.iter().chain(READ_ACCESSORS).copied().collect();
    // The walk tells a read through the door from one on a shape.
    let door =
        "at.view.load().caches(shape)\n(program.facts())\n    .caches(shape)\nfacts.caches(a)";
    assert!(cell_reads(door, &accessors).is_empty());
    assert_eq!(cell_reads("view.shape().caches()", &accessors).len(), 1);
    assert_eq!(cell_reads("(f(shape)).caches()", &accessors).len(), 1);
    let mut offenders = Vec::new();
    for reader in READERS {
        let source = manifest.join(reader);
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        for found in cell_reads(&code, &accessors) {
            offenders.push(format!("{reader}:{found}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "read a cell past `LoadFacts`:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_static_pass_s_rules_never_ask_which_half_they_run_over() {
    use crate::tests::boundary::{collect_sources, mode_branches, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = vec![manifest.join("src/dispatch/statics.rs")];
    collect_sources(&manifest.join("src/dispatch/statics"), &mut sources);
    let mut offenders = Vec::new();
    for source in sources.iter().filter(|source| {
        !["store.rs", "resume.rs", "entry.rs"]
            .iter()
            .any(|half| source.ends_with(half))
    }) {
        let code = strip_comments_and_strings(&std::fs::read_to_string(source).unwrap());
        for found in mode_branches(&code) {
            offenders.push(format!("{}:{found}", source.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "a rule asks which half it runs over:\n{}",
        offenders.join("\n")
    );
}

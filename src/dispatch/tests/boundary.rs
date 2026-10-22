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

/// The store's doors, which the walk's rules reach only through its façade, `walk/reads.rs`.
const ACCESSORS: &[&str] = &["binding", "fact", "close", "wakes", "write", "write_close"];

/// The fields of a level holding the facts the walk fixed, which its rules read and write only
/// through the façade.
const FIELDS: &[&str] = &[
    "declared",
    "registered",
    "expressions",
    "reads",
    "parts",
    "statements",
    "binders",
    "uses",
    "selections",
    "settled",
    "instances",
    "named",
    "trees",
    "throughs",
    "minting",
    "signature",
];

/// The walk's rule files: every file of `walk/` but the façade. `walk.rs` itself is the driver,
/// which enters and leaves the units and lays their records.
fn rules() -> Vec<std::path::PathBuf> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    crate::tests::boundary::collect_sources(&manifest.join("src/dispatch/walk"), &mut sources);
    sources.retain(|source| !source.ends_with("reads.rs"));
    sources
}

#[test]
fn the_walk_s_rules_reach_every_fact_through_its_facade() {
    use crate::tests::boundary::{store_bypasses, strip_comments_and_strings};
    // The scan bites on a store door and on a level's field.
    let bypasses = "self.store.close(segment)
self.chain[level].binders
self.closed(segment)";
    assert_eq!(store_bypasses(bypasses, ACCESSORS, FIELDS).len(), 2);
    let mut offenders = Vec::new();
    for source in rules() {
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        for found in store_bypasses(&code, ACCESSORS, FIELDS) {
            offenders.push(format!("{}:{found}", source.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "read past the façade:\n{}",
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

/// The cell accessors a run's reader might call on a shape: what `LoadFacts` answers in their
/// place, and `shape_at`, which the shape table answers.
const READ_ACCESSORS: &[&str] = &[
    "declared_type",
    "registered_type",
    "callable_type",
    "in_body",
    "resolution",
    "born_instance",
    "group_levels",
    "typed_expression",
    "type_expressions",
    "type_captures",
    "path_reads",
    "path_read",
    "path_entry",
    "paths",
    "through_at",
    "binder_type",
    "value_type",
    "refusal",
    "fix_born_instance",
    "path_closed",
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
    let accessors = READ_ACCESSORS;
    // The walk tells a read through the door from one on a shape.
    let door =
        "at.view.load().caches(shape)\n(program.facts())\n    .caches(shape)\nfacts.caches(a)";
    assert!(cell_reads(door, accessors).is_empty());
    assert_eq!(cell_reads("view.shape().caches()", accessors).len(), 1);
    assert_eq!(cell_reads("(f(shape)).caches()", accessors).len(), 1);
    let mut offenders = Vec::new();
    for reader in READERS {
        let source = manifest.join(reader);
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        for found in cell_reads(&code, accessors) {
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
fn the_walk_s_rules_never_ask_which_half_they_run_over() {
    use crate::tests::boundary::{mode_branches, strip_comments_and_strings};
    let mut offenders = Vec::new();
    for source in rules() {
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
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

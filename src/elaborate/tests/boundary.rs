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

/// The cell accessors and write doors the type channel's walk reaches only through its store.
const ACCESSORS: &[&str] = &[
    "declared_type",
    "registered_type",
    "typed_expression",
    "type_expressions",
    "path_spellings",
    "typed",
    "in_body",
    "refusal",
    "channel_residual",
    "fix",
    "fix_declared",
    "fix_registered",
    "fix_callable",
    "fix_group_levels",
    "fix_path_reads",
    "fix_channel_paths",
    "fix_declared_variables",
    "fix_channel_residual",
    "refuse_typing",
];

#[test]
fn the_type_channel_reaches_every_cell_through_its_store() {
    use crate::tests::boundary::{store_bypasses, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("src/elaborate/channel.rs");
    let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
    let offenders = store_bypasses(&code, ACCESSORS, &[]);
    assert!(
        offenders.is_empty(),
        "read past the store:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_type_channel_s_rules_never_ask_which_half_they_run_over() {
    use crate::tests::boundary::{mode_branches, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("src/elaborate/channel.rs");
    let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
    // The scan bites on each spelling of a mode branch, and passes a call named for a resumption.
    let branches = "if self.over.is_some() {}\nlet writer = self.writer;\nself.resume\n\
                    self.pass.over\nself.channel\nresume_channel()\nself.channel_entries()";
    assert_eq!(mode_branches(branches).len(), 6);
    let offenders = mode_branches(&code);
    assert!(
        offenders.is_empty(),
        "a rule asks which half it runs over:\n{}",
        offenders.join("\n")
    );
}

//! The import boundary, as a test over this module's own source.
//!
//! `dispatch` stands over the program and every layer below it — `builtin_image`, `elaborate`,
//! `knot`, `memory`, `parse`, `program`, `scheduler`, `scope`, `source`, `symbols`, `type_lattice`
//! and `values` — and nothing names it but the embedder.

/// The path prefixes `dispatch` may name.
const PREFIXES: &[&str] = &[
    "crate::builtin_image",
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
    "src/program/settle.rs",
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

/// The fact accessors a run's reader might call on a shape: what `LoadFacts` answers in their
/// place, and `shape_at`, which the shape table answers.
const READ_ACCESSORS: &[&str] = &[
    "declared_type",
    "registered_type",
    "callable_type",
    "in_body",
    "born_instance",
    "group_levels",
    "typed_expression",
    "type_expressions",
    "type_captures",
    "path_reads",
    "path_read",
    "path",
    "paths",
    "binder_type",
    "value_type",
    "refusal",
    "fix_born_instance",
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
    "statement_type",
    "use_statics",
    "view_slots",
    "view_tree",
    "warm_at",
];

#[test]
fn the_run_reads_every_load_fact_through_the_read_door() {
    use crate::tests::boundary::{fact_reads, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let accessors = READ_ACCESSORS;
    // The walk tells a read through the door from one on a shape.
    let door =
        "at.view.facts().caches(shape)\n(program.facts())\n    .caches(shape)\nfacts.caches(a)";
    assert!(fact_reads(door, accessors).is_empty());
    assert_eq!(fact_reads("view.shape().caches()", accessors).len(), 1);
    assert_eq!(fact_reads("(f(shape)).caches()", accessors).len(), 1);
    let mut offenders = Vec::new();
    for reader in READERS {
        let source = manifest.join(reader);
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        for found in fact_reads(&code, accessors) {
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

/// The lattice's judgements and the composites built from them: outside the doors
/// ([`judge`](crate::elaborate::judge)) a rule calls none, so every judgement over a type that may
/// hold a run-bound name marks.
const JUDGEMENTS: &[&str] = &[
    "judge_by_class(",
    "fits(",
    "admits_with(",
    "lower_end_outside(",
    "bound_above(",
    "scheme_bound_above(",
    "instance_under(",
    "select_by_class(",
    "intervals(",
    "retyped_to(",
    "never_satisfies(",
    "read_through(",
    "construction(",
];

/// The methods that judge: on a collector, an interval, or a type's handle.
const JUDGING_METHODS: &[&str] = &[
    ".solve(",
    ".reproducible(",
    ".concrete(",
    ".concrete_declared(",
    ".is_exact(",
    "types.node(",
];

/// Each judgement `code` makes past the doors: a free judgement not called as `judge::…`, or a
/// judging method.
fn judgements_past_the_doors(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for name in JUDGEMENTS {
        for (at, _) in code.match_indices(name) {
            let before = &code[..at];
            let joined = before
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.');
            if !joined && !before.ends_with("judge::") && !before.ends_with("fn ") {
                found.push(name.to_string());
            }
        }
    }
    for method in JUDGING_METHODS {
        found.extend(
            code.match_indices(method)
                .map(|(_, method)| method.to_string()),
        );
    }
    found
}

/// The walk's and the elaborator's files: every rule a load's typing runs, the doors' own file
/// and the tests aside.
fn judging_sources() -> Vec<std::path::PathBuf> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = vec![
        manifest.join("src/dispatch/walk.rs"),
        manifest.join("src/dispatch/rules.rs"),
    ];
    crate::tests::boundary::collect_sources(&manifest.join("src/dispatch/walk"), &mut sources);
    crate::tests::boundary::collect_sources(&manifest.join("src/elaborate"), &mut sources);
    sources.retain(|source| {
        let path = source.to_string_lossy();
        !path.ends_with("judge.rs") && !path.ends_with("tests.rs") && !path.contains("/tests/")
    });
    sources
}

#[test]
fn the_walk_judges_through_its_doors() {
    use crate::tests::boundary::strip_comments_and_strings;
    // The scan bites on a bare judgement, a judging method and a path around the doors, and lets
    // a door pass.
    let planted = "fits(types, scratch, a, b)
types.concrete(ty)
crate::type_lattice::bound_above(types, scratch, ty)
judge::fits(self, types, scratch, a, b)
sig_fits(types, scratch, a, b)";
    assert_eq!(judgements_past_the_doors(planted).len(), 3);
    let mut offenders = Vec::new();
    for source in judging_sources() {
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        for found in judgements_past_the_doors(&code) {
            offenders.push(format!("{}: {found}", source.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "judged past the doors:\n{}",
        offenders.join("\n")
    );
}

/// Each call of a store's [`answer`](crate::scope::Store::answer) in `code`: the witness's door.
fn witness_asks(code: &str) -> usize {
    code.matches(".answer(").count()
}

/// The witness is reached only through the marks: of the judging sources, the stores and `scope`,
/// only the walk's marks (`walk/reads.rs`) ask a store for its answer, once, so no rule reads a witness's
/// answer except through a door, and none reads its types at all.
#[test]
fn the_witness_is_asked_only_through_the_marks() {
    use crate::tests::boundary::strip_comments_and_strings;
    // The scan bites on a call and lets a definition pass.
    let planted = "self.store.answer(question, &unknown_of, types, scratch)\nfn answer(";
    assert_eq!(witness_asks(planted), 1);
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = judging_sources();
    sources.push(manifest.join("src/elaborate/judge.rs"));
    crate::tests::boundary::collect_sources(&manifest.join("src/program"), &mut sources);
    crate::tests::boundary::collect_sources(&manifest.join("src/build"), &mut sources);
    crate::tests::boundary::collect_sources(&manifest.join("src/scope"), &mut sources);
    sources.retain(|source| {
        let path = source.to_string_lossy();
        !path.ends_with("tests.rs") && !path.contains("/tests/")
    });
    let mut asks = Vec::new();
    for source in sources {
        let code = strip_comments_and_strings(&std::fs::read_to_string(&source).unwrap());
        let found = witness_asks(&code);
        if found > 0 {
            asks.push((source.to_string_lossy().into_owned(), found));
        }
    }
    assert!(
        asks.len() == 1 && asks[0].0.ends_with("src/dispatch/walk/reads.rs") && asks[0].1 == 1,
        "the witness is asked past the marks: {asks:?}"
    );
}

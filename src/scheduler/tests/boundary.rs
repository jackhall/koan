//! The import boundary, the storage discipline and the step's public surface, as tests over this
//! module's own source.
//!
//! `scheduler` may name `knot`, `memory`, `scope`, `type_lattice` and `values` and nothing else in
//! the crate — no `parse`, no `elaborate`; outside its tests it holds no owning heap type
//! but its own runtime state, which is never a value in a region. A step names no handle and no
//! carrier door.

/// The path prefixes `scheduler` may name.
const PREFIXES: &[&str] = &[
    "crate::knot",
    "crate::memory",
    "crate::scheduler",
    "crate::scope",
    "crate::tests::allocation_count",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::type_lattice",
    "crate::values",
];

/// The owning types outside the tests: the buffer a step pushes its requests into, the drain's
/// ready stack, the build arena's slots and free list, and the shared hold each settlement's facts
/// keep on their parent's. Each is the scheduler's runtime state, held beside the graph and never
/// written into a region, so a value's no-drop-glue discipline does not reach them.
const OWNING_ALLOWED: &[(&str, &str)] = &[
    ("src/scheduler/action.rs", "requests: Vec<Asked<'graph, B>>"),
    ("src/scheduler/action.rs", "Vec::new()"),
    ("src/scheduler/drain.rs", "ready: Vec<Entry<'graph, B>>"),
    ("src/scheduler/drain.rs", "Vec::new()"),
    ("src/scheduler/arena.rs", "slots: Vec<Slot<'graph>>"),
    ("src/scheduler/arena.rs", "use std::rc::Rc;"),
    ("src/scheduler/arena.rs", "Rc<Settled<'graph>>"),
    ("src/scheduler/arena.rs", "Rc::new(facts)"),
    ("src/scheduler/arena.rs", "free: Vec<u32>"),
    ("src/scheduler/arena.rs", "Vec::new()"),
];

/// The cellgraph doors no drain opens between steps: a peek into a cell's header, an entry beside
/// another cell, and a lend naming a cell.
const BETWEEN_STEPS: &[&str] = &["peek", "enter_beside", "create_tenant_lent", "Lend::Named"];

/// What no public method of `Step` may spell: a handle, a carrier door, or a carrier.
const STEP_MAY_NOT_NAME: &[&str] = &[
    "CellHandle",
    "TreeHandle",
    "SlabHandle",
    "alloc_into",
    "alloc_here",
    "lift",
    "keep",
    "redeem",
    "receipt",
    "Ready<",
    "Dormant<",
    "Operand<",
];

#[test]
fn scheduler_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("scheduler", PREFIXES, OWNING_ALLOWED);
}

/// Only the scheduler and `values` open an `ArenaRef` or make one from a slot: every other module
/// holds and passes the token, so a step can hand the drain only an arena node the scheduler named
/// to it.
#[test]
fn only_the_scheduler_and_values_open_an_arena_ref() {
    let source = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut files = Vec::new();
    crate::tests::boundary::collect_sources(&source, &mut files);
    let opening = |file: &std::path::Path| {
        ["scheduler", "values"].iter().any(|module| {
            file.starts_with(source.join(module)) || file == source.join(format!("{module}.rs"))
        })
    };
    let offenders: Vec<_> = (files.iter())
        .filter(|file| !opening(file))
        .filter(|file| {
            let text = std::fs::read_to_string(file).expect("a source file reads");
            let code = crate::tests::boundary::strip_comments_and_strings(&text);
            code.contains("ArenaRef::of") || code.contains(".opened()")
        })
        .collect();
    assert!(files.len() > 100, "the walk found the crate's sources");
    assert!(
        offenders.is_empty(),
        "a module outside the scheduler opens an `ArenaRef`: {offenders:?}"
    );
}

/// The drain keeps settlements as books beside the cell graph, so between steps it reaches no cell
/// but through a birth or a release: every `Fresh` cell borrows its parent's scratch and every
/// `Shares` one is its host's tenant.
#[test]
fn the_drain_opens_no_cellgraph_door_between_steps() {
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/scheduler"));
    let mut files = vec![root.with_extension("rs")];
    crate::tests::boundary::collect_sources(&root, &mut files);
    let offenders: Vec<_> = (files.iter())
        .filter(|file| !file.components().any(|part| part.as_os_str() == "tests"))
        .flat_map(|file| {
            let text = std::fs::read_to_string(file).expect("a source file reads");
            let code = crate::tests::boundary::strip_comments_and_strings(&text);
            (BETWEEN_STEPS.iter())
                .filter(|door| code.contains(*door))
                .map(|door| format!("{}: {door}", file.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(files.len() > 5, "the walk found the scheduler's sources");
    assert!(
        offenders.is_empty(),
        "the scheduler opens a cellgraph door between steps: {offenders:?}"
    );
}

#[test]
fn a_step_names_no_handle_and_no_carrier_door() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/scheduler/action.rs");
    let source = std::fs::read_to_string(path).expect("action.rs reads");
    let signatures = step_signatures(&source);
    assert!(
        signatures
            .iter()
            .any(|signature| signature.contains("fn finish_fresh")),
        "the walk found `Step`'s methods: {signatures:?}"
    );
    let offenders: Vec<&String> = signatures
        .iter()
        .filter(|signature| {
            STEP_MAY_NOT_NAME
                .iter()
                .any(|word| signature.contains(word))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "a public method of `Step` names a place or a carrier door: {offenders:?}"
    );
}

/// Every `pub fn` signature of the `impl` blocks over `Step`, each joined onto one line and cut at
/// the body's opening brace: from each `impl<` line whose header names `Step<` to the `}` closing
/// it at column zero. `Step` has one block per typestate, so the walk reads them all.
fn step_signatures(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let starts = (0..lines.len()).filter(|&index| {
        lines[index].starts_with("impl<")
            && lines[index..(index + 2).min(lines.len())]
                .iter()
                .any(|line| line.contains("Step<"))
    });
    let mut signatures = Vec::new();
    for start in starts {
        let end = (start..lines.len())
            .find(|&index| lines[index] == "}")
            .expect("the impl block closes");
        let mut current: Option<String> = None;
        for line in &lines[start..end] {
            let line = line.trim();
            if line.starts_with("pub fn") {
                current = Some(String::new());
            }
            if let Some(signature) = current.as_mut() {
                signature.push_str(line);
                signature.push(' ');
                if line.ends_with('{') {
                    signatures.push(current.take().expect("a signature in progress"));
                }
            }
        }
    }
    signatures
}

//! The run's roots as koan-held edges: `run_program` wires one edge per top-level statement, reads
//! the drain boundary through those edges, and releases every one of them before the harness — and
//! with it the run frame they name as their destination — tears down.

use super::*;
use crate::machine::program_storage;
use crate::parse::parse;

/// After `run_program` returns, no slab edge is still outstanding: the root edges it minted are
/// released on the way out, and every edge the run's own parks were wired through went with its
/// slot. A leaked root would be a name outliving the frame it was destined at, which is exactly
/// what the release-before-teardown ordering exists to prevent.
fn assert_edges_all_released(test_run: &TestRun<'_>, source: &str) {
    let scheduler = test_run.runtime.scheduler();
    // Non-vacuity: the run really did wire edges, so the equality below is a release claim rather
    // than a statement about an empty slab.
    assert!(
        scheduler.edge_slab_len() >= 3,
        "expected at least one slab edge per top-level root, got {}",
        scheduler.edge_slab_len(),
    );
    assert_eq!(
        scheduler.edge_free_list_len(),
        scheduler.edge_slab_len(),
        "every slab edge should be released after running {source:?} \
         ({} of {} free)",
        scheduler.edge_free_list_len(),
        scheduler.edge_slab_len(),
    );
}

/// Run `source` through the real `run_program` door, which mints and releases the root edges,
/// rather than the dispatch-then-execute shape this suite's other helper takes.
fn run_program_in<'run>(
    program: &'run ProgramStorage,
    region: &'run Rc<FrameStorage>,
    source: &str,
) -> (TestRun<'run>, Result<(), crate::machine::KError>) {
    let mut test_run = TestRun::silent(program, region);
    let root = test_run.scope;
    let exprs = parse(program.brand(), &test_run.registries().labels, source)
        .expect("parse should succeed");
    let outcome = test_run.runtime.run_program(root, exprs);
    (test_run, outcome)
}

#[test]
fn run_program_releases_every_root_edge() {
    let program = program_storage();
    let region = run_root_storage();
    let source = "LET x = 1\nLET y = (x)\nPRINT y";
    let (test_run, outcome) = run_program_in(&program, &region, source);
    outcome.expect("program should run");
    assert_edges_all_released(&test_run, source);
}

/// The error path releases too — the release sits outside the fallible middle precisely so an
/// erroring run cannot leave a root edge naming a dying run frame.
#[test]
fn run_program_releases_root_edges_on_the_error_path() {
    let program = program_storage();
    let region = run_root_storage();
    let source = "LET x = 1\nundefined_name";
    let (test_run, outcome) = run_program_in(&program, &region, source);
    outcome.expect_err("the unbound name should surface");
    assert_edges_all_released(&test_run, source);
}

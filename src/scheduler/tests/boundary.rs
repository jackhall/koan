//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `scheduler` may name `function`, `memory` and `values` and nothing else in the crate — no
//! `scope`, no `parse`, no `elaborate`; outside its tests it holds no owning heap type but its own
//! runtime state, which is never a value in a region.

/// The path prefixes `scheduler` may name.
const PREFIXES: &[&str] = &[
    "crate::function",
    "crate::memory",
    "crate::scheduler",
    "crate::tests::boundary",
    "crate::tests::case_share",
    "crate::values",
];

/// The owning types outside the tests: the drain's own work queues and the buffer a step pushes
/// its requests into. Each is the scheduler's runtime state, held beside the graph and never
/// written into a region, so a value's no-drop-glue discipline does not reach them.
const OWNING_ALLOWED: &[(&str, &str)] = &[
    ("src/scheduler/action.rs", "requests: Vec<Request<'graph>>"),
    ("src/scheduler/action.rs", "Vec::new()"),
    ("src/scheduler/drain.rs", "in_flight: VecDeque<CellHandle>"),
    ("src/scheduler/drain.rs", "fresh: VecDeque<CellHandle>"),
    ("src/scheduler/drain.rs", "VecDeque::new()"),
];

#[test]
fn scheduler_names_only_the_layers_below_it_holds_no_heap_and_spells_the_stack_lifetimes() {
    crate::tests::boundary::holds("scheduler", PREFIXES, OWNING_ALLOWED);
}

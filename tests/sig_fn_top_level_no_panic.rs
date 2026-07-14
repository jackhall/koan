//! Integration test for the scheduler-reclaim-vs-interpret-readback fix: a
//! top-level `SIG` followed by an `FN` whose signature references the SIG (both
//! as parameter type and return type) used to panic at the interpret seam's
//! per-top-level `read_result`, because the FN-def's dep-finish was installing the
//! SIG's top-level slot as an `Owned` dep and cascade-freeing it on success.
//!
//! Runs the smoke through `interpret_with_writer` (the same path the CLI uses)
//! rather than `Scheduler::execute` directly, because the unit-test path does
//! not exercise the per-top-level read-back. Asserts only that interpret
//! returns `Ok` — the test is a regression pin against the panic.

use koan::machine::interpret_with_writer;

#[test]
fn top_level_sig_then_fn_referencing_it_runs_without_panic() {
    let source = "SIG Ordered = (VAL compare :Number)\n\
         FN (MAKESET Er :Ordered) -> Ordered = (Er)";
    interpret_with_writer(source, Box::new(std::io::sink()))
        .expect("top-level SIG + FN program should run without panic or error");
}

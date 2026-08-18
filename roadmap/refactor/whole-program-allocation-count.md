# Whole-program allocation counting

**Problem.** The counting instrument is test-scoped. `src/tests.rs` installs a
`#[global_allocator]` that bumps a thread-local tally, read through
`allocation_count()` ([src/tests.rs](../../src/tests.rs)), so a `#[cfg(test)]` test can
bracket one call and assert a delta. It cannot report allocations for a whole
program run: it is thread-local, it exists only in the lib-test build, and it
replaces the binary's `mimalloc` ([src/main.rs](../../src/main.rs)), so a wall-clock
reading off a test build measures a different allocator than the one that ships.
Every claim about per-dispatch and per-step allocation traffic on the execute path is
therefore a static read of the call graph, not a measurement, and no change to that
traffic can be shown to have moved a number.

**Acceptance criteria.**

- A whole-program allocation count is available behind a cfg on the binary target,
  reporting total allocations for a program run without displacing `mimalloc` in a
  normal build.
- Two recorded shapes have a committed baseline count: a tail-recursive loop (step
  churn) and a long operator chain (per-dispatch churn).
- A regression test asserts the per-run count for at least one of those shapes stays
  within a stated bound, so a re-introduced allocation on the execute path fails a
  test rather than going unnoticed.

**Directions.**

- *Cfg shape — open.* A cargo feature that swaps the global allocator for the counting
  one, versus keeping `mimalloc` and counting through a wrapper allocator that
  delegates. Recommended: the delegating wrapper, so the counted build and the shipped
  build share an allocator and wall-clock stays comparable.
- *Counter scope — open.* Thread-local as today versus a process-wide atomic. The
  execute path is single-threaded, so thread-local suffices for the two recorded
  shapes; a process-wide count is what makes the number reportable for an arbitrary
  program.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Step-scoped scratch arena](step-scratch-arena.md) — the counts decide whether the arena's plumbing earns its keep.

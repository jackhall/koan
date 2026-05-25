# Continue-on-error for the REPL and batch mode

**Problem.** A top-level failure ends the session: the scheduler surfaces the
`KError` to the CLI, which formats it to stderr and stops. A REPL therefore
cannot keep running past a typo, and the CLI's batch mode aborts at the first
failed expression instead of running the next one.
[`interpret`](../../src/machine/execute/interpret.rs) and
[`Scheduler::execute`](../../src/machine/execute/scheduler.rs) return
`Result<(), KError>`, so the first error propagates all the way out.

**Impact.**

- *REPL survives a bad expression.* A top-level failure no longer ends the
  session; the prompt returns and the next expression still runs.
- *Batch mode runs the whole file.* The CLI reports each failed expression and
  continues, so a single error no longer masks the rest of the run.

**Directions.**

- *Per-expression continuation boundary — open.* Where the continuation seam
  sits: a loop in the CLI driver that catches each top-level expression's
  `KError`, versus a scheduler-level mode that isolates statement nodes so one
  errored slot doesn't poison siblings. The `add_catch` primitive that backs
  [`CATCH`](../../design/error-handling.md) already isolates a sub-dispatch's
  fault, so the driver-loop form may reuse it.
- *Error reporting in continue mode — open.* Whether a continued run collects
  errors and prints a summary at the end, or streams each to stderr as it
  happens. Batch mode and an interactive REPL may want different answers.

## Dependencies

**Requires:** none.

**Unblocks:** none.

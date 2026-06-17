# Continue-on-error for the REPL and batch mode

**Problem.** A top-level failure ends the session: the scheduler surfaces the
`KError` to the CLI, which formats it to stderr and stops. A REPL therefore
cannot keep running past a typo, and the CLI's batch mode aborts at the first
failed expression instead of running the next one.
[`interpret`](../../src/machine/execute/runtime/interpret.rs) and
[`Scheduler::execute`](../../src/machine/execute/run_loop.rs) return
`Result<(), KError>`, so the first error propagates all the way out.

**Acceptance criteria.**

- After a top-level expression raises a `KError` in the REPL, the prompt
  returns and a subsequent expression evaluates normally.
- In batch mode, every top-level expression in a file is attempted; a failed
  expression is reported and execution proceeds to the next one.

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

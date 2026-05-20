# Error-handling surface follow-ups

Surface-level error-handling work remaining after the `Result` builtin and
`CATCH` shipped. The items share this file because the type-system and
dispatcher decisions for one constrain the others. See
[design/error-handling.md](../../design/error-handling.md) for the shipped
substrate — the `Result` type, `CATCH`, `TRY-WITH`, and the privilege-boundary
principle between interpreter faults and user errors.

**Problem.** A top-level failure ends the session, so the CLI's batch mode and a
REPL both stop at the first error instead of running the next expression.
`Result` has no stdlib combinators, so user code chaining fallible computations
writes MATCH boilerplate by hand for every step. And the bare-leaf arm of
`elaborate_type_expr` rejects identifier-class names in type-position slots with
a diagnostic that doesn't name the surface "modules-as-types" layering (per
[design/typing/functors.md](../../design/typing/functors.md)), so the error
reads as an internal failure rather than a spelling correction.

**Impact.**

- *REPL ergonomics.* A top-level failure no longer ends the session; the next
  expression still runs.
- *Composable `Result` handling.* `map` / `bind` / `unwrap_or` let user code
  thread a value through fallible steps without a hand-written MATCH at each
  one, so typed-error returns become ergonomic rather than verbose.
- *Self-correcting type diagnostics.* A name used in a type-position slot that
  resolves only in the value language points the user at the Type-class
  spelling convention instead of surfacing a scheduler-internal message.

**Directions.**

- *stdlib `Result` helpers — deferred.* `map`, `bind`, `unwrap_or`, and friends
  are Koan source, not builtins, so they are gated on
  [files and imports](files-and-imports.md) and the
  [standard library](standard-library.md). The `Result` constructor itself is
  builtin and already shipped, so user code can construct and MATCH on `Result`
  before these land.
- *Continue-on-error in the REPL — open.* A top-level failure currently ends the
  session; the CLI's batch mode should keep going. Independent of the
  type-system work.
- *"Type-language binder expected" diagnostic vocabulary — open.* The bare-leaf
  arm of `elaborate_type_expr` rejects identifier-class names in type-position
  slots (shipped substrate; see
  [design/typing/elaboration.md](../../design/typing/elaboration.md)). The
  diagnostic needs vocabulary that names the surface "modules-as-types" layering
  (per [design/typing/functors.md](../../design/typing/functors.md)) without
  leaking scheduler internals. Pick wording — candidates: "type-language binder
  expected", "name is value-language only", or similar — with a hint pointing at
  the Type-class spelling convention.

## Dependencies

**Requires:** none — continue-on-error and the diagnostic vocabulary each ship
independently; the stdlib `Result` helpers are the only gated piece.

**Unblocks:** none.

The stdlib `Result` helper module (`map`, `bind`, `unwrap_or`, …) is gated on
[files and imports](files-and-imports.md) and
[standard library](standard-library.md), but that gating is sub-item-local — the
other two items have no prerequisites.

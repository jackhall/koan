# Migrate result-slot value reads to `open`

Restructure the result-slot value reads that escape a reference up-stack onto `open` + copy-out,
so the value path no longer needs a returned borrow.

**Problem.** With the result slot stored as [`Sealed`](sealed-open.md), `read_result` /
`read_result_with_frame` are rerouted internally but still hand a re-anchored reference back to
their callers, which carry it up the dispatcher call stack. That returned borrow is the shape
`open`-only forbids; until each such caller copies out or inverts into a closure, `attach` cannot
be removed from the value path.

**Acceptance criteria.**

- Every result-slot value read that currently rides a re-anchored reference up-stack either copies
  the needed value out of the `open` closure or is restructured CPS so the consumption nests inside
  it; no value-path borrow escapes its access window.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Per-site copy-out vs CPS — open.* Each consumer chooses copy-out (a cheap value) or a
  continuation rewrite (a borrow-heavy path); decided site-by-site during implementation.

## Dependencies

**Requires:**

- [Sealed node-storage carrier and `open`](sealed-open.md) — supplies the `open` accessor and the
  sealed result slot these reads convert onto.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — clearing the value-path escapes is one of the four
  migrations that must land before `attach` can be deleted.

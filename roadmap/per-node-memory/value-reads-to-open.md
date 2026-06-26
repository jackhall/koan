# Migrate result-slot value reads to `open`

Restructure the result-slot value reads that escape a reference up-stack onto `open` + copy-out,
so the value path no longer needs a returned borrow.

**Problem.** With the result slot stored as [`Sealed`](../../src/witnessed.rs), `read_result` /
`read_result_with_frame` are rerouted through the transitional self-witnessed
[`read`](../../src/witnessed.rs) but still hand a re-anchored reference back to their callers, which carry
it up the dispatcher call stack. That returned borrow is the shape `open`-only forbids; until each
such caller copies out or inverts into a closure, the transitional `read` cannot be deleted, so
`Sealed` keeps a second access verb on the value path.

**Acceptance criteria.**

- Every result-slot value read that currently rides a re-anchored reference up-stack either copies
  the needed value out of the `open` closure or is restructured CPS so the consumption nests inside
  it; no value-path borrow escapes its access window.
- With its callers inverted, the transitional self-witnessed [`read`](../../src/witnessed.rs) is
  deleted from `Sealed`, leaving the value path on `open` alone.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Per-site copy-out vs CPS — open.* Each consumer chooses copy-out (a cheap value) or a
  continuation rewrite (a borrow-heavy path); decided site-by-site during implementation.
- *This item owns the `read` deletion — decided.* The self-witnessed `read` is retired here (the
  item that clears its callers), the dual of [remove-attach](remove-attach.md) retiring the
  externally-witnessed `attach`; the two land the single-access-verb end-state together.

## Dependencies

**Requires:**


**Unblocks:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — one of the call sites that
  item surveys for an un-nestable non-scope reference.
- [Remove `attach`](remove-attach.md) — clearing the value-path escapes is one of the
  migrations that must land before `attach` can be deleted.

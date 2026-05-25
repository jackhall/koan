# Scheduler eager-free policy vs. interpret top-level read-back

Top-level `SIG ...\nFN (... :SigName) -> ...` programs panic at
[`node_store.rs:169`](../src/machine/execute/scheduler/node_store.rs)'s
`read_result` *"result must be ready by the time it's read"* assertion
when run through the [`interpret`](../src/machine/execute/interpret.rs)
seam. Pinned by the
[`functor_returning_bare_signature_typed_param_does_not_panic`](../src/builtins/fn_def/tests/functor/dual_write.rs)
regression test's comment block; the unit-test path
(`test_support::run` → `Scheduler::execute`) does not call
`read_result` per top-level id, so the regression is invisible to
`cargo test` and surfaces only via `cargo run`.

**Problem.** The scheduler's
[`Scheduler::reclaim_deps`](../src/machine/execute/scheduler.rs)
walks `DepGraph::owned_children` and frees the consumer's owned-edge
producers as soon as the consumer terminalizes. For a top-level `FN`
that combines on a `SIG`'s placeholder, the FN-def's Combine succeeds
and `reclaim_deps` eagerly frees the SIG dispatch's slot — including
its `NodeOutput`. Interpret then walks each top-level id and calls
`NodeStore::read_result(top_level_id)` for printing; the SIG's slot is
no longer in the `results` vector, so the read hits a `Free` slot and
panics.

The unit-test path does not exercise the read-back, so the panic
doesn't fire in tests:
[`Scheduler::execute`](../src/machine/execute/scheduler/execute.rs)
drains to a fixed point and returns without per-top-level
`read_result`. Only [`interpret`](../src/machine/execute/interpret.rs)
walks top-level ids and prints their results, so the smoke crash
surfaces only at the CLI boundary.

**Impact.**

- *Top-level SIG plus signature-typed FN programs become writable end to
  end.* The smoke
  `SIG OrderedSig = (VAL compare :Number)\nFN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)`
  runs through `cargo run` without panicking.
- *The dual-`execute`/`interpret` correctness gap closes.* Tests and
  the CLI share one slot-lifecycle contract; a slot that's safe to
  read after `execute` returns is also safe to read after `interpret`
  walks results.
- *Removes a load-bearing caveat from the regression test.* The
  [`functor_returning_bare_signature_typed_param_does_not_panic`](../src/builtins/fn_def/tests/functor/dual_write.rs)
  test's comment block can drop the interpret-seam-panic note.
- *Foundation for any future post-execute slot inspection.* Editor
  tooling and debuggers that walk slot results after a run lean on
  the same invariant the interpret read-back wants.

**Directions.**

- *Fix locus — open.* Two viable shapes:
  - *Interpret holds Owned edges per top-level id.* The
    [`interpret`](../src/machine/execute/interpret.rs) loop registers
    each top-level dispatch as an Owned consumer of itself (or of a
    synthetic anchor slot), defeating the cascade by keeping
    `pending_deps > 0` until the read happens. Lossless for the
    common case (no extra walk in `execute`); requires a new
    "external owner" registration surface on `Scheduler`.
  - *`reclaim_deps` filters top-level dispatches.* The reclaim walk
    treats top-level ids as roots and skips the free even when the
    consumer terminalizes. Surgical (one branch in `reclaim_deps`)
    but adds a "top-level-ness" flag to slot state that's otherwise
    a scheduler invariant.
  - *Recommended:* the Owned-edge approach — it expresses the
    contract ("the caller still wants to read this slot") in the
    same vocabulary `reclaim_deps` already uses, rather than adding
    a scheduler-side exception.
- *Regression test surface — decided.* A new integration test under
  `tests/` that runs the smoke through the same `interpret_with_writer`
  path the CLI uses, asserting the printed output rather than poking
  at scheduler internals. The existing unit-test path can't reach the
  read-back, so the regression must live at the integration seam.

## Dependencies

**Requires:**

**Unblocks:**

- [FUNCTOR binder](type_language/functor-binder.md) — FUNCTOR's
  panic-blocker footnote folds into this item once the read-back path
  runs clean, since the panic the binder work was carrying was always
  the scheduler's eager-free policy, not the type-language layer.

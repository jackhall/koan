# Migrate scope-handle reads to `open`

Restructure the scope-handle reads that escape an `&Scope` up-stack onto `open` + copy-out, so the
decide layer no longer carries a re-anchored scope reference.

**Problem.** After the shipped FrameStorage restructure (see
[memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure))
the per-call child scope is an externally-witnessed `Sealed`, read through
`scope_bounded` / `current_scope` /
`reattach_node_scope`. Those readers still hand a re-anchored `&Scope` back to the decide layer,
which carries it through `run_dispatch` / `SchedulerView` — the escaping borrow `open`-only
forbids. Until each such reader copies out or inverts into a closure, `attach` cannot be removed
from the scope path.

**Acceptance criteria.**

- Every scope-handle read that currently rides a re-anchored `&Scope` up-stack either copies the
  needed data out of the `open` closure or is restructured CPS so the consumption nests inside it;
  no scope-path borrow escapes its access window.
- TCO frame reuse is unaffected — `try_reset_for_tail` still passes its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Per-site copy-out vs CPS — open.* As with the [value reads](value-reads-to-open.md), each
  reader chooses copy-out or a continuation rewrite, decided site-by-site.

## Dependencies

**Requires:**


**Unblocks:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — one of the call sites that
  item surveys for an un-nestable non-scope reference.
- [Remove `attach`](remove-attach.md) — clearing the scope-path escapes is one of the
  migrations that must land before `attach` can be deleted.

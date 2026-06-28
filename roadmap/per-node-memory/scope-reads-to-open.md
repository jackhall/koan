# Invert the scope-handle reads onto `open`

Restructure the scope-handle reads that re-anchor an `&Scope` up the dispatcher stack onto the
existing CPS readers, rework `Region::alloc`'s bare-arena re-anchor off the free wrapper, and delete
`reattach_ref_with`.

**Problem.** The result-slot *value* reads now nest under [`Sealed::open`](../../src/witnessed.rs)
(the value-read migration shipped), but the scope channel still hands a re-anchored `&Scope` back up
the call stack — the shape the rank-2 `open` forbids by construction. The scope-handle reads
(`current_scope` / `reattach_node_scope` /
[`CallFrame::scope_bounded`](../../src/machine/core/arena.rs)) carry an `&Scope` through
`run_dispatch` / [`SchedulerView`](../../src/machine/execute/dispatch/ctx.rs), and `Region::alloc`'s
read-back plus the two [`scope_ptr`](../../src/machine/core/scope_ptr.rs) handles re-anchor through the
loose [`reattach_ref_with`](../../src/witnessed.rs) wrapper (4 sites). Until each inverts onto a CPS
reader, the wrapper stays alive as an alternate spelling of the one primitive, and `attach`
([a single access verb](single-open-verb.md)) keeps its callers.

**Acceptance criteria.**

- Every scope-handle read that currently rides a re-anchored `&Scope` up-stack copies the needed data
  out of the access or is restructured CPS so the consumption nests inside it; no scope borrow escapes
  its access window.
- `Region::alloc` expresses its bare-arena re-anchor without the free `reattach_ref_with` wrapper.
- `reattach_ref_with` is deleted, and no call site references it.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Rework `Region::alloc` onto `Witnessed` — decided.* The bare-arena re-anchor expresses through the
  witnessed substrate rather than the free wrapper. **Open risk:** the scope chain holds raw `NonNull`
  ([`BoundedScopePtr`](../../src/machine/core/scope_ptr.rs)), so routing `alloc` through `Witnessed` is
  a substrate re-architecture, not a local rewrite — the concrete design is TBD and is surfaced before
  the wrapper is deleted.
- *Per-site copy-out vs CPS — open.* Each scope reader chooses copy-out (a cheap field) or a
  continuation rewrite; decided site-by-site during implementation.

## Dependencies

Builds on the shipped value-read migration (the result-slot value reads nest under `Sealed::open`),
which leaves the scope channel as the remaining re-anchored read.

**Requires:** none — the value-read migration shipped; this item inverts the scope channel onto the
same `open`.

**Unblocks:**

- [`Sealed`: a single access verb](single-open-verb.md) — clearing the scope-read escapes (and the
  `reattach_ref_with` wrapper that backs `attach`'s callers) is what must land before `attach` can be
  deleted.

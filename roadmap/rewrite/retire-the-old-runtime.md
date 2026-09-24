# Retire the old runtime

The last step of the rewrite: delete the first implementation and the crate it
scheduled on, once every layer above them has been rebuilt.

**Problem.** The first runtime — [src/machine/](../../src/machine),
[src/builtins/](../../src/builtins), [src/main.rs](../../src/main.rs), the
`tests/*.rs` integration binaries and the `workgraph` crate's
[src/](../../workgraph/README.md) — sits behind the `pending_rewrite` cargo
feature, and that build no longer compiles
([TEST.md](../../TEST.md#the-pending-rewrite)). It is read as requirements, not
run. Two things keep it on disk anyway. The first is that the rewrite has not
reached it: `machine` still holds the only implementation of the layers the
remaining items rebuild. The second is that deleting it is a documentation
problem as much as a code one — [old_design/](../../old_design/README.md) is
frozen and never edited, yet carries eighty-odd links into `workgraph/src/**`
and as many into `src/machine/**`, and `workgraph`'s own
[DAG-scheduler design](../../workgraph/old_design/dag-scheduler.md) and
[old_roadmap/](../../workgraph/old_roadmap/README.md), the requirements record
that is meant to survive the delete, carry fifty more into the code being
deleted. `src/lib.rs` also still re-exports `workgraph::scheduler` under the
name koan's own [scheduler](../../src/scheduler/README.md) module now holds, so
the two collide in the `pending_rewrite` build.

**Acceptance criteria.**

- `src/machine/`, `src/builtins/`, `src/builtins.rs`, `src/main.rs` and the
  `tests/*.rs` integration binaries are gone, and `src/lib.rs` declares no
  module and re-exports nothing behind a `pending_rewrite` gate.
- The `pending_rewrite` cargo feature is gone, as is every feature that only
  turns the old runtime on, and no script, doc or CI invocation names one.
- The `workgraph` crate is gone from the workspace and from
  `[workspace] members`; its `old_design/` and `old_roadmap/` trees remain as
  the requirements record, and no invocation names `workgraph/test-hooks`.
- Every doc link that named a deleted source file resolves: `doclinks check` is
  green across `old_design/`, `workgraph/old_design/`,
  `workgraph/old_roadmap/`, `roadmap/old_*/`, `audit/`, `observe/`, `TEST.md`
  and `README.md`.
- `README.md`'s source layout and `TEST.md` describe a tree with one runtime in
  it, and neither names a build that does not compile.

**Directions.**

- *What a frozen doc's link into deleted code becomes — open.* `old_design/` is
  requirements reading for exactly the code this item deletes, and the
  partition forbids editing it. Options: repoint each link at the kept module
  that replaced the named file; strip the link and keep the prose, since the
  prose is the requirement and the path was only ever evidence; or give
  `doclinks` a rule that a retired tree's links are not gated. The choice
  applies identically to `workgraph/old_design/` and `workgraph/old_roadmap/`,
  whose links point into the crate half being deleted.
- *Both halves of the old runtime go together — decided.* `src/machine/**` and
  `workgraph/src/**` are named by the same frozen docs in the same way, so
  whatever answers the question above answers it for both; deleting one half
  first buys nothing and splits the doc sweep in two.
- *Whether the old runtime is read one last time first — open.* The rewrite's
  items each measured themselves against `old_design/`, not against the code.
  If nothing is left that only the code records, the delete is unconditional;
  if something is, it is written into the module README that owns it before the
  delete rather than after.

## Dependencies

The item is a delete, so it requires the rewrite to have rebuilt everything the
deleted code implements; the leaves of that chain are the three below.

**Requires:**

- [Modules](modules.md) — the module surface `machine` still owns.
- [Control expression shapes and errors](control-and-errors.md) — the control surface `machine` still owns.
- [Yielding iterators](yielding-iterators.md) — the last of the execution
  surface `workgraph`'s DAG layer still owns.

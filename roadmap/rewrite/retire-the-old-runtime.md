# Retire the old runtime

Delete the first implementation, the crate it scheduled on, and the documents
that only described them.

**Problem.** The first runtime — [src/machine/](../../src/machine),
[src/builtins/](../../src/builtins), the `tests/*.rs` integration binaries and
the `workgraph` crate — sits behind the `pending_rewrite` cargo feature, and
that build no longer compiles ([TEST.md](../../TEST.md#the-pending-rewrite)).
The features the rewrite has left to build differ enough from it that nothing
reads it as reference any more. A doc tree written about it keeps it on disk:
[old_design/](../../old_design/README.md), `workgraph`'s own docs, and the
`roadmap/old_*` projects together carry about a thousand links into the code,
and the live docs, tools and skills name the feature, its knobs and the
`workgraph/test-hooks` feature throughout. One `old_design/` section,
[label-interning.md § Names fixed in Rust source](../../old_design/label-interning.md#names-fixed-in-rust-source),
is the only statement of the kept `symbols` module's `static_name!` and
`slots!` declarations. `src/lib.rs` also still re-exports
`workgraph::scheduler` under the name koan's own
[scheduler](../../src/scheduler/README.md) module now holds, so the two collide
in the `pending_rewrite` build.

**Acceptance criteria.**

- `src/machine/`, `src/builtins/`, `src/builtins.rs`, the guard fixtures, the
  `tests/` directory and `audit/reach_audit*` are gone, and `src/lib.rs`
  declares no module and re-exports nothing behind a feature gate.
- The `pending_rewrite`, `seam-force-copy`, `seam-force-pin` and `region-audit`
  features are gone, and no script, doc, skill or CI invocation names one or
  `workgraph/test-hooks`.
- The `workgraph/` directory is gone, and the workspace has no `workgraph`
  member.
- `old_design/` and `observe/miri_slate_pending_rewrite.md` are gone. The
  `symbols` module's README states how a name fixed in Rust source is declared.
- Of the `roadmap/old_*` projects, only
  [two-phase execution](../old_editor_tooling/two-phase-execution.md) remains,
  and it carries no link into a deleted path.
- `doclinks check` is green, and `README.md`, `TEST.md`, `AGENTS.md` and the
  skills describe a tree with one runtime in it.
- Every test in a kept module that reached the old runtime is deleted, and no
  allocation-bound regression test replaces `tests/allocation_baseline.rs`.

**Directions.**

- *Both halves of the old runtime go together — decided.* `src/machine/**` and
  `workgraph/src/**` are described by the same frozen docs in the same way, so
  deleting one half first buys nothing and splits the doc sweep in two.
- *Whether the old runtime is read one last time first — decided.* No. The
  remaining features differ enough that it holds nothing they need. The one
  exception is the `symbols` section above. It describes kept code, so it moves
  into `src/symbols/README.md` before the delete.
- *What a link into deleted code becomes — decided.* None survives. The docs
  that carried such links are deleted with the code, and the live docs and the
  one surviving old item are edited by hand.

## Dependencies

**Requires:** none.

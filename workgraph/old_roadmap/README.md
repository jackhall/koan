# workgraph roadmap

> **Stale as work, kept as requirements.** Written against the old runtime and the crate it drove,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../../roadmap/rewrite/README.md) as it meets them.

Open work on the library stack: the `cellgraph` computation-cell substrate
(working name —
[cellgraph/README.md](../../cellgraph/README.md)) and the
`workgraph` DAG scheduler above it, whose public surface is memory-safe by
construction. The items here land the consumer API and move the boundary
incrementally; each is sized for one PR. Koan's own roadmap is
[roadmap/](../../roadmap/README.md), and the division of responsibility between
the two — what is library, what is embedder — is stated in
[old_design/scheduler-library.md](../../old_design/scheduler-library.md).

## Crossing the crate boundary

koan names workgraph's carrier and scheduler types across dozens of source files,
and those types are brand-parameterized: a koan-side adapter absorbing a signature
change would have to re-plumb every brand lifetime. There is no facade to hide a
boundary move behind, so a change to workgraph's surface is a change to koan's.

Items that move the boundary split in three, each landing on its own:

1. **Expand.** workgraph gains the new surface alongside the old. koan is
   untouched and still compiles, so this lands as a workgraph-only item —
   [sectioned reach](../old_design/sectioned-reach.md) was scoped this way,
   workgraph-side only.
2. **Migrate.** koan adopts the new surface, as a separate item in the project
   owning the callers — for sectioned reach, that was koan's own
   [value substrates](../../old_design/value-substrates.md#sectioned-reach).
3. **Contract.** workgraph deletes the superseded surface. This is an acceptance
   criterion on the migrate item rather than a follow-up: an expand that never
   contracts leaves two ways to say the same thing, and the second one outlives
   whoever remembers why it is there.

An expand may break koan outright when the old surface cannot be kept alongside
the new. That is allowed to land: the routine tier of `tools/verify.sh` detects a
workgraph-only change scope, runs the library slate, and reports koan's compile
state without gating on it. What is owed in exchange is step 2 — an expand that breaks koan
lands with its migrate item already written down, so the debt is tracked rather
than discovered at the next koan build.

## Items

Every requirements doc in this retired project.

- [Rebuilding workgraph over cellgraph](adopt-cellgraph.md)
- [Publishing the workgraph crate](workgraph-extraction.md)

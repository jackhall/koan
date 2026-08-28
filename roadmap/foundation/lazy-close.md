# Lazy close

**Problem.** The copy verb treats functions and modules as borrow leaves:
[`relocate_object_into`](../../src/machine/model/values/kobject.rs) carries
their references verbatim under either verb, so a copied value containing a
closure severs nothing for it, and [CLOSE OVER](close-over.md) can only pin
callable captures. Fat frames therefore survive every severance surface
through callable chains, and the escape seam cannot consolidate an escaping
closure — the exact lever the liveness matrix wants
([liveness-matrix.md](../../workgraph/design/liveness-matrix.md)).

**Acceptance criteria.**

- Deep-copying a function value whose environment is data-only rebuilds that
  environment at the destination: the product reaches no source region,
  observed through the memory substrate.
- Cycles and sharing are preserved: a recursive closure copies to a closure
  whose captured scope binds the copy itself; two closures over one defining
  scope copy to two closures over one copied scope.
- A copy that reaches an unfinalized binding or a not-yet-closed scope does
  not wait: that copy downgrades to `Pin` (always sound — retaining more
  never dangles), and consolidation retries at a later priced crossing —
  frame death at the latest, by which point the scope has closed. No park
  edge is added to the finalize walk, so the wait-cycle deadlock two
  mutually-referencing in-flight environments would create is
  unconstructible, not merely handled.
- A `CLOSE OVER` callable capture severs transitively: its data is copied,
  its own callable references recurse.
- The copy-or-recurse decision remains seam-priced; no definition-site cost
  is added.

**Directions.**

- *Mechanism — decided per [design/lazy-closures.md](../../design/lazy-closures.md).*
  Extend the `Copy` verb transitively through callable leaves: deep-copying
  a `KFunction` / `Module` rebuilds its captured scope chain at the
  destination — data bindings relocated under `Copy`, nested callables
  recursed — memoized per source scope and callable so the
  scope→function→scope cycle a recursive FN creates terminates and sibling
  closures sharing one defining scope share one copy. Eternal-homed scopes
  are referenced verbatim.
- *Trigger — decided.* The copy fires where reach evidence prices it (the
  escape seam's `copy_or_pin`-style decision), never unconditionally at
  definition sites: definition stays O(1); escape is the priced seam.
- *Unready environments — decided.* Pin, never park (the third acceptance
  criterion); no wait edges enter the finalize walk.

## Dependencies

**Requires:**

- [CLOSE OVER](close-over.md)

**Unblocks:** none directly; the liveness-matrix consolidation gate consumes
this when that design is planned.

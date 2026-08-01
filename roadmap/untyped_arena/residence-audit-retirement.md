# Residence-audit retirement

Retire the runtime residence walk in favor of the fold-brand construction
doors that already compile-enforce residence.

**Problem.** The runtime residence audit — the
[`Residence`](../../src/machine/core/arena/residence.rs) ownership predicate
and the evidence-tier `Scope` move-in doors fused onto it — vets every
composite move-in at runtime, re-checking what the fold-brand construction
door already compile-enforces:
[`FoldingBrand::alloc_object_folded`](../../src/machine/core/arena.rs) stores a
value built from a fold's declared operands with no runtime audit — an
ambient-lifetime capture is a compile error at the closure signature — and the
Rebuild adoption path
([`rebuild_delivered_substrate`](../../src/machine/core/scope/reach.rs))
already rides it end to end: the fold's composition mints and retains the
copy's reach, and the product envelope is the finished carrier. The Pin and
CopyNode dispositions (`store_object_pinned` / `store_object_adopted`, over
the shared `store_projection_reaching` / `store_value_reaching` audit) and the
module doors (`store_module_object`, `store_transparent_view`) still run the
walk, so the residence audit reads as enforcement rather than the stopgap it
is.

**Acceptance criteria.**

- The reaching/evidence-tier residence machinery is deleted, its move-ins
  routed through the fold-brand construction doors (`transfer_into_placing` +
  `FoldingBrand::alloc_object_folded`) that compile-enforce residence: the
  pin/adopt-reaching stores (`store_object_adopted` / `store_object_pinned`),
  their shared audit (`store_value_reaching`, `store_projection_reaching`), the
  module reaching stores (`store_module_object`, `store_transparent_view`), the
  `Residence` struct's reach-evidence arm (`with_reach` and the `covers_region`
  reach disjunct), `ResidenceEvidence::reaching`, and
  [`KObject::resident_in_delivered`](../../src/machine/model/values/kobject.rs).
- The only runtime residence checks remaining are the backstops
  [design/witness-hosting.md § Residence enforcement](../../design/witness-hosting.md#residence-enforcement)
  names — the primitive `ptr::eq` reattach guards (the `AuditedStored` impls
  for `KFunction` / `Scope` / `Module`) — plus the dest-only
  [`resident_in_visiting`](../../src/machine/model/values/kobject.rs) walk
  under `alloc_object_checked_stored` / `alloc_object_witnessed_checked`,
  which [drop-free region death](drop-free-region-death.md) deletes.
- The Miri audit slate is green after the retirements.

## Dependencies

The *retention* half of the same obligation already sits in the library's mint —
a mint is a retention, and folding reach into a region by hand is off the
embedder surface
([workgraph/design/reach.md § Composition](../../workgraph/design/reach.md#composition-minting-a-description-and-retaining-its-pins)).
This item consolidates the *residence* half into the library's construction
doors, leaving no memory-safety bookkeeping in embedder hands.

**Requires:** none — the fold-brand door and the placing transfer it routes
through are shipped
([design/witness-hosting.md § Residence enforcement](../../design/witness-hosting.md#residence-enforcement)).

**Unblocks:**

- [Drop-free region death](drop-free-region-death.md) — the reaching tier is
  retired here via the fold-brand doors; that item deletes the residual
  dest-only `resident_in_visiting` walk once the expression-part families are
  `Drop`-free, and migrates the families to the untyped bump arena.

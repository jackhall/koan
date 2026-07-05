# Region-hosted witness sets

Move witness-set storage into region arenas and shrink carriers to set
references, per [design/witness-hosting.md](../../design/witness-hosting.md).

**Problem.** A carrier's witness is an owned
[`RegionSet`](../../workgraph/src/witnessed/region_set.rs) — a
`Vec<Rc<FrameStorage>>` — so the set's bytes travel with every carrier:

- Every carrier clone (`Sealed::duplicate`, dep delivery, `transfer_into`)
  clones the `Vec` — a heap allocation even in the singleton common case — and
  bumps one refcount per member.
- Every carrier-oriented binding read
  ([`Bindings::lookup_value_carrier` / `lookup_type_carrier` /
  `lookup_member`](../../src/machine/core/bindings.rs)) clones the entry's
  stored `FrameSet` out per hit, on the interpreter's hottest path, because
  the entry owns its reach and the read must not hold the map borrow.
- Member `Rc` decrements scatter across individual carrier drops instead of
  batching at frame teardown, the level at which regions actually die.
- The scope-level reach accumulator
  ([`Scope.reach`](../../src/machine/core/scope.rs)) is a soundness-bearing
  ownership slot: `adopt_sealed`'s unsafe re-anchor rests on the scope holding
  folded `Rc`s until it drops — a scope-special pinning channel beside the
  carrier one.
- The scheduler threads the owned-set witness through the
  [`Workload::Witness`](../../workgraph/src/scheduler/workload.rs) associated
  type plus the `SetWitness` widening lift and the `sole()` singleton-recovery
  accessor, machinery that exists only because the witness is an owned set
  rather than a hosted reference.

**Acceptance criteria.**

- Witness sets are a `Stored` family: each region's storage bundle carries a
  witness-set sub-arena, every set a carrier references lives in exactly one
  region's arena, and a stored set exposes no mutation surface (composition
  mints a new set into a destination arena).
- Carrier clone and dep delivery perform no set allocation: the walking
  witness is one host `Rc` plus an erased set reference (or the frameless
  empty value), and duplicating a sealed terminal bumps exactly one refcount.
- Binding entries store a bare set reference into their own region's arena;
  the carrier-oriented lookups return without cloning a set.
- `Scope` holds no reach accumulator: adoption-style re-anchors are pinned by
  arena-hosted deposits, and a module's foreign reach is computed at scope
  close as the union over the child scope's binding entries' sets.
- The scheduler's terminal slots store the library's hosted witness type; the
  `Workload::Witness` associated type, the `SetWitness` lift, and the
  singleton-recovery accessor are gone from the library surface.
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Walking-form packaging — decided.* One owned host `Rc<F>` + erased set
  reference as a library type (working name `HostedWitness<F>`), per
  [design/witness-hosting.md](../../design/witness-hosting.md); bare set
  references are resident-form-only (a bare reference cannot pin its hosting
  region, which home-omission keeps out of the set).
- *Set immutability — decided.* Frozen at store; every union mints a new set
  into a destination arena whose allocation capability the mint verb takes by
  signature.
- *Module reach — decided.* Seal-time union over the child scope's binding
  entries at `close()`, excluding deposits (transient adoptions).
- *Per-region already-pinned dedup index — deferred.* Bounds deposits at
  O(distinct regions) instead of O(adoptions); pure footprint optimization,
  not soundness-bearing, so it lands only if deposit growth is observed.
- *Migration order — open.* (a) Host the sets first behind the existing
  owned-`FrameSet` API (regions gain the family; carriers still own handles),
  then swap carriers to references and delete the accumulator; (b) introduce
  `HostedWitness` first at the scheduler boundary, then migrate storage.
  Recommended: (a) — the arena and freeze rules are testable before any
  carrier-form change, and the accumulator deletion needs the arena in place.

## Dependencies

Soft ordering with [region-pure-empty-reach](region-pure-empty-reach.md):
hosting extends a set's life to its region's death, so that item's mint-time
precision should land with or before this one. Neither hard-requires the other.

**Requires:** none — the storage-bundle and allocation-capability substrate
has shipped.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — this item moves
  the library surface (`Workload::Witness`, the set machinery), so it lands
  before identifiers freeze.

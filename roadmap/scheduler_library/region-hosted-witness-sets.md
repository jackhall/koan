# Region-hosted witness sets

Move witness-set storage into region arenas and shrink carriers to set
references, per [design/witness-hosting.md](../../design/witness-hosting.md).

**Problem.** A carrier's witness is
[`CarrierWitness`](../../src/machine/core/carrier_witness.rs) — an owned
`{ pins: Vec<CarrierPin>, reach: FrameSet }` pair — so both halves' bytes
travel with every carrier:

- Every carrier clone (`Sealed::duplicate`, dep delivery, `transfer_into`)
  clones both vectors — heap allocations even in the singleton common case —
  and bumps one refcount per pin and per reach member.
- Every carrier-oriented binding read
  ([`Bindings::lookup_value_carrier` / `lookup_type_carrier` /
  `lookup_member`](../../src/machine/core/bindings.rs)) clones the entry's
  stored `FrameSet` out per hit, on the interpreter's hottest path, because
  the entry owns its reach ([`StoredReach`](../../src/machine/core/bindings.rs))
  and the read must not hold the map borrow; a read that materializes the
  borrows-into-home bit adds a singleton union on top.
- Member and pin `Rc` decrements scatter across individual carrier drops
  instead of batching at frame teardown, the level at which regions actually
  die.
- The scope holds two soundness-bearing ownership slots beside the carrier
  channel: the reach accumulator
  ([`Scope.reach`](../../src/machine/core/scope.rs)), whose folded `Rc`s back
  `adopt_sealed`'s unsafe re-anchor, and the deposit list (`Scope.deposit`),
  which keeps adopted carriers' severed owned backings alive for the scope's
  life.
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
  witness is one backing `Rc` — the host frame-owner, or a severed node's
  owned backing — plus an erased set reference (or the frameless empty value),
  and duplicating a sealed terminal bumps exactly one refcount.
- Binding entries store a bare set reference into their own region's arena;
  the carrier-oriented lookups return without cloning a set.
- `Scope` holds neither a reach accumulator nor a severed-backing deposit
  list: adoption-style re-anchors — foreign region members and severed owned
  backings alike — are pinned by arena-hosted deposits, and a module's foreign
  reach is computed at scope close as the union over the child scope's binding
  entries' sets.
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
- *Severed carriers under hosting — open.* The finalize sever produces a
  walking carrier backed by an owned node `Rc`
  ([`CarrierPin::Object` / `Type`](../../src/machine/core/carrier_witness.rs))
  instead of a host frame, and the sever gate keys on "reach does not cover
  the producer" — so a severed carrier can still carry non-empty foreign
  reach, whose hosted set would have no live home arena once the producer
  dies. (a) `HostedWitness` gains an owned-backing arm and the sever re-mints
  the set into a still-live region named by the reach; (b) narrow the sever
  gate to empty-reach values, so a reach-carrying value keeps its producer
  frame and the walking form stays exactly host + set. Recommended: (b) first
  — it keeps the packaging binary and the sever's payoff (freeing the frame at
  Done) already accrues to the region-pure common case; the cost is
  foreign-reaching, home-pure values reverting to pinning their producer
  frame.
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

Mint-time precision has shipped as
[`CarrierWitness`](../../src/machine/core/carrier_witness.rs) (the pins-vs-reach
split); this item generalizes its owned-`FrameSet` `reach` into the hosted,
home-omitted set reference, so it re-points that type at the walking-form
packaging (`HostedWitness`) described in
[witness-hosting.md](../../design/witness-hosting.md).

**Requires:** none — the storage-bundle and allocation-capability substrate
has shipped.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — this item moves
  the library surface (`Workload::Witness`, the set machinery), so it lands
  before identifiers freeze.

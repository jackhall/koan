# Delivery-driven frame retention

Move walking-carrier liveness onto scheduler frame-retention and collapse the
carrier to the single reference-only witness, per
[design/witness-hosting.md § Retention model](../../design/witness-hosting.md#retention-model).

**Problem.** A walking carrier owns its liveness — the host frame-owner `Rc` it
carries ([Host-pinned walking carrier](host-pinned-walking-carrier.md) leaves
that one arm in place) — so a producer frame's release is a function of carrier
drops scattered across consumers, not of the scheduler's deliveries, and every
carrier hand-off pays a refcount bump. Around that self-owned liveness:

- The finalize Done-boundary gate **conflates frame release with reach**: it
  decides whether to sever a value off its producer frame by asking whether the
  value's reach covers that frame
  ([`finalize.rs`](../../src/machine/execute/finalize.rs)), fusing a lifecycle
  decision with a reach fact.
- The scheduler threads the witness through the
  [`Workload::Witness`](../../workgraph/src/scheduler/workload.rs) associated
  type, the [`SetWitness`](../../workgraph/src/witnessed.rs) lift, and the
  [`sole()`](../../workgraph/src/witnessed/region_set.rs) singleton-recovery
  accessor.

**Acceptance criteria.**

- The scheduler retains a producer frame's owner until every destination has
  pulled its terminal, releasing at pull-count zero. Frame release is a
  function of deliveries only — never of any value's reach
  ([witness-hosting.md § The pinning invariant](../../design/witness-hosting.md#the-pinning-invariant),
  rule 4).
- Retention releases on every path a pull can die on, not just successful
  delivery: a consumer short-circuited by an errored dep, a freed or spliced
  node, and run teardown all decrement to release. No frame outlives its last
  possible pull (the Miri leak gate is the check).
- The carrier is a **single** reference-only witness,
  `{ borrows_host: bool, reach: &WitnessSet }` — no owned host arm, no
  severed-backing variant — used identically whether resident or walking. A
  clone is a bit-copy plus a reference-copy: no refcount traffic.
- No finalize sever gate remains, and `borrows_host` never influences a frame's
  lifetime: it is read at exactly one kind of site, the re-home mint into a
  different destination arena.
- A pure pass-through — a value returned up the call stack unmodified — rides by
  reference with zero allocation and zero refcount traffic; its birth frame
  stays retained until the value is re-homed or its last delivery is pulled.
- The scheduler's terminal slots store the library carrier; the
  `Workload::Witness` associated type, the `SetWitness` lift, and the `sole()`
  accessor are gone from the library surface.
- A tail loop's free ordering rides retention: the retiring incarnation's region
  is released at pull-count zero, after the reinstalled incarnation adopts the
  carried arguments.
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Retention by pull-count — decided,* per
  [witness-hosting.md § Retention model](../../design/witness-hosting.md#retention-model).
  The scheduler already knows every destination of a terminal (its notify /
  dep edges); retention is bookkeeping on the delivery path, not new graph
  structure. Release is a delivery fact; nothing severs.
- *Sever gate retired; pass-through rides its birth frame — decided.* The
  re-home mint (a bind into a longer-lived region) is the only place members
  move; retention covers the dwell in between. The severed-backing carrier
  variant and any bind-side severed handling from the two carrier items are
  deleted along with the gate.
- *`borrows_host` as reach representation only — decided.* Consumed only when a
  value's reach is minted into a different destination arena; never read for a
  lifecycle decision.

## Dependencies

Retention must meet a single region lifecycle: it lands after tail-call region
turnover is library-owned, so no Koan-side reserve or in-place reset competes
with the retained frames.

**Requires:**

- [Host-pinned walking carrier](host-pinned-walking-carrier.md) — supplies the
  hosted sets and the carrier whose owned arm this item deletes.
- [Library-owned tail-call region reuse](tco-library-region-reuse.md) — makes
  region lifecycle library-owned, so retention is the only release mechanism.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — this item moves
  the library surface (`Workload::Witness`, the set machinery), so it lands
  before identifiers freeze.

# Region-hosted witness sets

Host reach sets in region arenas and collapse the carrier to a single
reference-only witness, per
[design/witness-hosting.md](../../design/witness-hosting.md).

**Problem.** A carrier's witness is
[`CarrierWitness`](../../src/machine/core/carrier_witness.rs) — an owned
`{ pins: Vec<CarrierPin>, reach: FrameSet }` pair — and several soundness-bearing
mechanisms are tangled around it:

- Every carrier clone (`Sealed::duplicate`, dep delivery, `transfer_into`) clones
  both vectors — a heap allocation even in the singleton case — and bumps one
  refcount per pin and per reach member.
- Every carrier-oriented binding read clones the entry's stored `FrameSet` out per
  hit, on the interpreter's hottest path, because the entry owns its reach and the
  read must not hold the map borrow.
- [`Scope`](../../src/machine/core/scope.rs) holds two soundness-bearing ownership
  slots beside the carrier channel: a reach accumulator (`Scope.reach`) and a
  deposit list (`Scope.deposit`) that keeps adopted carriers' owned backings alive.
- The finalize Done-boundary gate **conflates frame release with reach**: it
  decides whether to sever a value off its producer frame by asking whether the
  value's reach covers that frame
  ([`finalize.rs`](../../src/machine/execute/finalize.rs)), fusing a lifecycle
  decision with a reach fact.
- The scheduler threads the owned-set witness through the `Workload::Witness`
  associated type plus the `SetWitness` lift and the `sole()` accessor.

**Acceptance criteria.**

- Witness sets are a `Stored` family: each region's storage bundle carries a
  witness-set sub-arena, every set a carrier references lives in exactly one
  region's arena, and a stored set exposes no mutation surface (composition mints a
  new set into a destination arena).
- The carrier is a **single** witness, `{ borrows_host: bool, reach: &WitnessSet }`
  — no arms, no pins-vs-reach split, no owned host `Rc` — used identically whether
  resident or walking. A clone is a bit-copy plus a reference-copy: no set
  allocation, no refcount bump.
- Frame release is driven only by delivery: the scheduler retains a producer frame
  until every destination has pulled its terminal, releasing at pull-count zero. No
  finalize sever gate remains, and `borrows_host` never influences a frame's
  lifetime.
- A pure pass-through — a value returned up the call stack unmodified — allocates
  no set and re-homes nothing; its carrier rides by reference. A mint runs only
  where a value is bound into a different destination arena, where `borrows_host`
  materializes the old home into that set.
- `Scope` holds neither a reach accumulator nor a deposit list: it stores the
  hosted carrier directly, and a bind mints the value's reach into the scope's home
  arena.
- The scheduler's terminal slots store the library carrier; the `Workload::Witness`
  associated type, the `SetWitness` lift, and the singleton-recovery accessor are
  gone from the library surface.
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Single reference-only carrier — decided.* `{ borrows_host, reach: &WitnessSet }`
  hosted in the value's own region's arena; owns no `Rc`. Resident and walking
  share it; liveness is external (container, or frame-retention). Per
  [design/witness-hosting.md](../../design/witness-hosting.md).
- *Frame release by delivery — decided.* The scheduler retains a producer frame
  until every destination pulls its terminal (release at pull-count zero). Nothing
  severs; `borrows_host` is a reach-representation bit only, materialized at
  re-home.
- *Scopes store hosted carriers — decided.* No accumulator, no deposit list; a bind
  mints the reach into the scope's arena, folding both old jobs into the one
  resident set.
- *Set immutability — decided.* Frozen at store; every union mints a new set into a
  destination arena whose allocation capability the mint verb takes by signature.
- *Module reach — decided.* Seal-time union over the child scope's binding entries
  at `close()`.
- *TCO under library-owned regions — open.* A tail loop reuses one frame in
  constant space and resets its arena, but a value hosted in that frame (e.g. a
  closure tail-returned up the stack) references a set in that arena, and the
  frame's `Rc` is held by the scheduler's retention rather than by Koan — so Koan
  cannot unilaterally reuse or reset it. Candidate directions to weigh once the
  reuse mechanism ([`try_reset_for_tail`](../../src/machine/core/arena.rs)) is
  characterized: (a) re-home the values carried into the next iteration before
  reset; (b) forbid reuse of a frame that still has un-pulled terminals; (c) a
  library reuse-with-relocation verb Koan calls through the region capability. Needs
  investigation before any is chosen.

## Dependencies

The reach-set type [`RegionSet`](../../workgraph/src/witnessed/region_set.rs)
exists; this item hosts it in region arenas, references it from the single carrier,
migrates `Scope` and the scheduler surface onto it, and adds frame-retention — the
model [witness-hosting.md](../../design/witness-hosting.md) describes.

**Requires:** none — the storage-bundle and allocation-capability substrate has
shipped.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — this item moves the
  library surface (`Workload::Witness`, the set machinery), so it lands before
  identifiers freeze.

# Host-pinned walking carrier

Collapse the walking carrier to a hosted set reference plus one owned host arm,
per [design/witness-hosting.md § The carrier](../../design/witness-hosting.md#the-carrier).
The follow-up, [Delivery-driven frame retention](delivery-driven-frame-retention.md),
deletes that arm once region lifecycle is library-owned.

**Problem.** A walking carrier — a sealed terminal in a node slot, a dep
crossing steps — is still the owned
[`CarrierWitness`](../../src/machine/core/carrier_witness.rs)
`{ pins: Vec<CarrierPin>, reach: FrameSet }` pair even though binding entries are
now hosted (per
[design/witness-hosting.md § Scope and bindings](../../design/witness-hosting.md#scope-and-bindings-above-the-substrate),
which migrated only the resident side):

- Every carrier clone —
  [`Sealed::duplicate`](../../workgraph/src/witnessed.rs), dep delivery,
  [`transfer_into`](../../workgraph/src/witnessed.rs) — clones both vectors, a
  heap allocation even in the singleton case, and bumps one refcount per pin
  and per reach member.
- The seal/bind boundary converts between two witness representations: sealing
  a resident value rebuilds owned pins from its hosted set's members, and the
  bind mint folds them back into a hosted set.

**Acceptance criteria.**

- A walking carrier is `{ borrows_host: bool, host: Rc<F>, reach: &WitnessSet }`
  (`F` is the workload's frame-owner type — `FrameStorage` in Koan). The set
  reference is stored lifetime-erased and re-anchored under the held host pin,
  the same erase/reattach discipline values ride
  ([witnessed.rs](../../workgraph/src/witnessed.rs)); the host arm is what
  covers it, and through the set's members, every region the value reaches
  ([witness-hosting.md § The pinning invariant](../../design/witness-hosting.md#the-pinning-invariant),
  rule 2).
- The `pins: Vec<CarrierPin>` representation is gone: a walking carrier holds
  exactly one liveness arm — the host `Rc` (frame-backed) or the severed
  variant's owned node `Rc` (per the direction below). A frame-backed clone is
  a bit-copy, one refcount bump, and a reference-copy: no set allocation, no
  per-member refcount traffic.
- A pure pass-through — a value returned up the call stack unmodified — mints no
  set and re-homes nothing: its carrier is handed up with host and set
  reference unchanged. A mint runs only where a value is bound into a different
  destination arena, where `borrows_host` materializes the old home into that
  set.
- Severed values (the finalize sever's frame-free backings) stay sound and
  leak-free, and declared returns keep releasing the callee frame promptly —
  the behavior [finalize.rs](../../src/machine/execute/finalize.rs) implements
  today.
- The scheduler compiles with
  [`Workload::Witness`](../../workgraph/src/scheduler/workload.rs)
  re-instantiated to the collapsed carrier; the associated type itself, the
  `SetWitness` lift, and the finalize sever gate all remain (they retire with
  [Delivery-driven frame retention](delivery-driven-frame-retention.md)).
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Walking form keeps one owned host arm — decided.* The host pin is the
  transitional liveness source; nothing external retains a producer frame yet,
  so the carrier must. [Delivery-driven frame retention](delivery-driven-frame-retention.md)
  deletes the arm once the scheduler retains producer frames itself.
- *Severed backings walk as a second variant — decided.* The finalize sever
  produces a frame-free owned backing (`CarrierPin::Object` / `::Type`), a
  hosted set for such a value has no live host arena to sit in, and severed
  values do carry non-empty foreign reach (a value that borrows ancestors but
  not its own frame; the type channel's severed backing survives the
  declared-return re-stamp). So the walking form is two variants — frame-backed
  `{ host: Rc<F> }` with the hosted set reference, and severed
  `{ node: Rc<…>, reach: RegionSet<F> }` with an owned reach — and the severed
  variant is transitional debt deleted with the sever gate in
  [Delivery-driven frame retention](delivery-driven-frame-retention.md). At
  bind, a severed value re-homes into the scope's arena
  ([`Scope::adopt_sealed`](../../src/machine/core/scope.rs)). A narrowed sever
  (empty-foreign-reach values only) is rejected: values that borrow foreign
  regions but not their own frame would stop severing and instead pin their
  producer frame into the binder's scope for its whole life — an interim
  memory regression against today's prompt frame release.

## Dependencies

Delivery-driven retention and the final reference-only carrier are deliberately
out of scope: they land after tail-call region turnover is library-owned, so
retention meets a single region lifecycle. The finalize sever gate's *decision*
(does the value's reach cover its producer frame?) is unchanged here — it now
reads `borrows_host` instead of a set query, but stays a lifecycle gate until
the retention item removes it.

**Requires:**


**Unblocks:**

- [Library-owned tail-call region reuse](tco-library-region-reuse.md) — the
  sealed arguments and kept-first contract ride the single host-pinned carrier.
- [Delivery-driven frame retention](delivery-driven-frame-retention.md) —
  supplies the hosted sets and the carrier whose owned arm that item deletes.

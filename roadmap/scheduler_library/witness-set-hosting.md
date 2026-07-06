# Witness-set hosting substrate

Give witness sets a home in region storage: the hosting family and the mint
verbs, per [design/witness-hosting.md](../../design/witness-hosting.md). Purely
additive — no carrier, scope, or scheduler path migrates here; this is the
substrate the resident and walking migrations
([Resident hosted carriers](resident-hosted-carriers.md),
[Host-pinned walking carrier](host-pinned-walking-carrier.md)) compose.

**Problem.** A reach set is an owned value composed by cloning. The set type is
[`RegionSet<F>`](../../workgraph/src/witnessed/region_set.rs) — Koan
instantiates it as `FrameSet = RegionSet<FrameStorage>`
([arena.rs](../../src/machine/core/arena.rs)) — and every union (a bind fold, a
dep fold, a transfer between regions) allocates a fresh owned set and bumps one
refcount per member `Rc`. No region stores a set: the storage bundle
([`StorageProfile::Families`](../../src/machine/core/arena.rs), the `(K, Rest)`
cons-list) has no witness-set family, there is no frozen stored form, and no
composition path targets a destination arena.

**Acceptance criteria.**

- Witness sets are a [`Stored`](../../workgraph/src/witnessed/region.rs) family:
  each region's storage bundle carries a witness-set sub-arena, declared in the
  embedder's `Families` cons-list and bound through a `Stored::cell` impl
  exactly like the existing `KObject` / `Scope` families in
  [arena.rs](../../src/machine/core/arena.rs) — no parallel storage engine.
- A stored set is frozen: it exposes no mutation surface, and reading its
  members is a pinned read through the same erase/reattach substrate values use
  ([witnessed.rs](../../workgraph/src/witnessed.rs)).
- Mint verbs compose one or more source sets into a destination arena and are
  the only way a stored set comes to exist. Each verb takes the destination's
  allocation capability by signature (a
  [`RegionHandle`](../../workgraph/src/witnessed/region.rs) or the step
  context's region) and applies the three rules of
  [witness-hosting.md § Composition](../../design/witness-hosting.md#composition-minting-a-set):
  home-omission (the destination's own region is never a member),
  borrows-host materialization (a source carrier's host bit becomes a concrete
  member when its old host is foreign to the destination), and outer-chain
  subsumption through the workload's
  [`PinsRegion`](../../workgraph/src/witnessed/region_set.rs) hook.
- The mint reads its source members precisely: a minted set's members come from
  the source sets' exact member lists, never from "everything the host region
  reaches."
- The substrate is exercised end to end by tests — sets minted across regions,
  members read back under a pin, teardown releasing members at region death —
  and the full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Hosted, frozen, per-object sets — decided,* per
  [witness-hosting.md § The shape](../../design/witness-hosting.md#the-shape).
  Per-object precision (no whole-region merged set), freeze-at-store, members
  released only at the hosting region's teardown. The stored form may reuse
  `RegionSet<F>` or wrap it in a sealed stored type — identifiers are working
  names; the shape is the commitment.
- *Mint mechanics — decided,* per
  [witness-hosting.md § Composition](../../design/witness-hosting.md#composition-minting-a-set).
  Mechanism is library code in
  [workgraph/src/witnessed/](../../workgraph/src/witnessed.rs); the
  home-omission predicate at bind sites arrives as a caller-supplied closure
  (the embedder's policy — see the resident item), not as library policy.
- *Subsumption hook workload-supplied — decided.* Mechanism library-owned,
  member semantics via the embedder's existing `PinsRegion` impl on
  `FrameStorage` (its `outer`-chain walk).

## Dependencies

Scope for the implementer: this item adds types, one family, and verbs plus
their tests. `CarrierWitness`, `Scope`, binding reads, sealing, and the
scheduler are all untouched — if a diff here reaches those files, it has grown
past this item.

**Requires:** none — the storage-bundle and allocation-capability substrate has
shipped.

**Unblocks:**

- [Resident hosted carriers](resident-hosted-carriers.md) — binds mint into the
  scope's arena through these verbs.

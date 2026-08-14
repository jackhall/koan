# Delivery at finalize

The flip at the heart of edge-centric delivery: terminals distribute into
destination regions the moment they exist, and the consumer-pull machinery
deletes. Lands as one cross-crate item — koan's consuming sites convert in the
same landing, so every commit is full-repo green.

**Problem.** Delivery is consumer-pull: a terminal parks in `SlotState::Done`,
consumers read `dep_delivered` at step start and adopt into their own region,
and the window between finalize and the last read is bridged by scheduler-held
bookkeeping — the `owed` late-pull channel, the `Owned`/`Notify` edge kinds,
`resolve_alias` walks over `Aliased` residuals, and the partial discharge verbs
(`reclaim_deps`, `free`'s triage, the free cascade) in
[dep_graph.rs](../src/scheduler/dep_graph.rs) and
[node_store.rs](../src/scheduler/node_store.rs). This is workgraph holding
region liveness hostage to node accounting. The pinned design
([dag-scheduler.md § Delivery at finalize](../design/dag-scheduler.md#delivery-at-finalize),
[reach.md § Retention model](../design/reach.md#retention-model)) delivers at
the finalize walk and keeps no scheduler-held pins.

**Acceptance criteria.**

- The finalize notify-walk delivers per edge: released edges skip; live edges
  bucket by destination-region pointer via a look-back scan (no allocation);
  one adopt per distinct destination; the resident fans out to every edge in
  the bucket; each consumer's `pending` decrements per edge. Errors deliver
  cloned per destination under `W::Error: Clone`.
- Slots reclaim at finalize once their notify drains, unconditionally.
  `SlotState::Done` and `Aliased` are gone as at-rest states; `resolve_alias`
  and `dep_delivered` are gone.
- The retention hold, its finalize-time seeding, the standing-destination
  count, the `owed` channel, the `Owned`/`Notify` edge kinds, and the partial
  discharge verbs (`reclaim_deps`, `free`'s triage, the free cascade) are
  deleted.
- Install's filled branch reads the edge's resident — lifted under the
  destination's owner via the region host back-link — and the same-destination
  shortcut shares the resident
  ([dag-scheduler.md § Late wiring and install](../design/dag-scheduler.md#late-wiring-and-install)).
- Alias splice re-points parked edges once; no `Aliased` rows survive as
  residuals ([dag-scheduler.md § Alias splice](../design/dag-scheduler.md#alias-splice)).
- The destination deref is the one crate-internal `unsafe` site, with the
  containment lattice (destination outlives owner outlives edge) as its
  written witness argument and the debug-only weak shadow asserted live.
- Koan: deps arrive as ordinary residents of the consumer's region — the
  step-start dep fold in
  [run_loop.rs](../../src/machine/execute/run_loop.rs) and `run_step`'s
  `owned_deps` / `reclaim_deps` plumbing delete; the `ForwardReady` relocation
  and declared-return sites in
  [runtime.rs](../../src/machine/execute/runtime.rs) become install-time
  delivery; drain-boundary root reads are resident reads of koan's own regions.
- `NodeId` is a drive-loop currency only — the embedder pops, steps, wires,
  and may do graph surgery with it, and it carries a release-build allocation
  stamp so equality survives finalize-time slot recycling. Koan's machine core
  (scopes, bindings, frames) stores no `NodeId`; declaration identity rides
  the stamp. The `Delivered` envelope leaves the embedder *read* surface
  (internal transit inside the walk; `finalize` still receives one) — edges
  hand out the sealed resident cell instead.
- Workgraph Miri slate timelines: delivery under copy and pin verdicts,
  consumer death before its producer fires (released-edge skip), late wire to
  a filled edge (same- and cross-destination), root-edge release at the drain
  boundary.

**Directions.**

- Staging — decided. One cross-crate item, internally two full-repo-green
  commits: the flip plus koan consuming residents, then the deletion sweep.
- Adopt capability — decided: `&self` throughout; the deref yields `&Region`
  and crate-private `RegionHandle::new` mints the capability.
- Per-destination dedup — decided, ruled 2026-08-12; the destination-free
  notify-only edge variant is rejected.
- Error representation — decided: `W::Error: Clone`, satisfied by `KError`.
- Delivery relocation — decided, 2026-08-13: a `Workload::deliver` hook runs
  the embedder's relocation seam (deepcopy-vs-pin via `still_borrows`) at each
  distinct destination; koan's impl is `relocate_seam`.
- Owned-dep edges — decided, 2026-08-13: the install door mints them
  internally, destined at the consumer's anchor region; submission wrappers
  keep returning `NodeId`. The `Deps` currency collapse stays in
  [deps-currency-collapse.md](../../roadmap/refactor/deps-currency-collapse.md).
- Reinstall handoff — deferred to
  [reinstall-delivery-at-replace.md](reinstall-delivery-at-replace.md); the
  `handoff` hold stays vestigial-but-harmless through this item.

## Dependencies

**Requires:** none — koan already speaks edges and wires through the install
door, so the flip lands deep-and-narrow.

**Unblocks:**

- [Delivery at replace for reinstallation](reinstall-delivery-at-replace.md) —
  delivery-at-wiring is what makes the handoff hold vestigial.
- [Collapse the Deps owned/park currency](../../roadmap/refactor/deps-currency-collapse.md) —
  the scheduler stops consuming the split here.
- [Carving the cellgraph crate](cellgraph-extraction.md) — the carve happens
  against the settled slot/edge substrate.

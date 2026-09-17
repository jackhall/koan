# Shared regions, a scratch habitat and region recycling

The three mechanisms a copy-avoiding scheduler needs from the substrate: many
cells in one region, memory a cell can prove it throws away, and a dead cell's
chunks served to the next birth.

**Problem.** Every cell owns exactly one region, and the substrate names the
opposite composition as its only one:
[README.md § What the substrate is](../README.md#what-the-substrate-is-and-what-it-refuses-to-be)
gives one unit of work two regions by composing cells, and has no way to give
many units of work one region. Three gaps follow for an embedder that passes
destinations rather than moving built values.

- *A value built for another cell embeds nothing of the builder's.*
  `alloc_into`'s build is `for<'cell>` ([src/graph.rs](../src/graph.rs)), so a
  `'here` capture is readable inside it and never embeddable, whatever the
  destination. A sub-dispatch whose result shares structure with its arguments
  — a cons onto an existing tail, a slot pointing at a frame-homed value —
  re-enters each argument as a priced operand on every wake, or takes a
  `Crossing::Forced` deep copy into a sibling.
- *A region has no throwaway half.* A long-lived cell's multi-step transients
  land in the same bump as what it keeps, and stay until the cell dies. The
  only reset the crate has is its own verb-entry `Scratch`
  ([src/scratch.rs](../src/scratch.rs)), `pub(crate)` and strictly inside a
  step. `'severed` enforces "may not be embedded" for one build closure and
  does not outlive it.
- *`release` hands a region's chunks back to the allocator* and `create` asks
  the allocator for fresh ones, so a release-plus-create tail loop pays an
  allocation per hop for storage the previous hop just gave up.

**Acceptance criteria.**

- `create_tenant` makes a cell as a **tenant** of a live host cell, named by its
  own handle kind: it has no region of its own, `writer()` in its step is minted
  from the host's bump, and `executing_dest()` reports the host. A value a
  tenant writes embeds a host-homed borrow with no operand, no pin and no price.
- A live tenant named as a host, a tree parent or a placement destination
  resolves to its host, whether or not the host's own death has been declared. A
  test runs a chain of tenants each created by naming the one before it, after
  the first host's release, and ends with an empty graph.
- A tenant's death is a count decrement on the host — no reclaim, no splice, no
  pledge, no tombstone — and the host's region is reclaimed only when the host
  is dead and its tenant count is zero. A test holds a host's `'here` borrow
  across a park during which a tenant appends to the host's bump, and reads it
  back.
- A tenant's carriers carry the host's reach, and redeem entitlement for a
  tenant step is the host's.
- `StepContext` carries a `'scratch` brand with `'here: 'scratch`, quantified
  per `enter`; `scratch_writer()` returns a `Writer<'scratch>` over a second
  bump per region, and a scratch value may reference `'here` storage.
- A scratch structure survives a park through a second continuation slot
  (`store_scratch_successor` / `scratch_continuation`) over its own family
  parameter, which defaults to the continuation family, with `Reattachable`
  unchanged.
- Both continuation halves are re-anchored in `enter`, and neither
  `continuation()` nor `scratch_continuation()` contains `unsafe`.
- Embedding a scratch borrow into storage is a compile error by every route,
  each pinned by a `compile_fail` test checked against a compiling control:
  `store_successor`, `writer()` output used at `'here`, `lift`, an `alloc_into`
  build (foreign destination and the executing cell itself), the return of
  `enter`, and `Cell::set` on a `'here`-homed slot.
- The scratch bump is reset at `enter` when the scratch slot is empty, with no
  failable check. For a shared region the scratch bump is the host's and the
  reset waits on every tenant's scratch slot being empty.
- A tree child's death absorbing into a parent that holds named scratch leaves
  the parent's scratch intact, and the departing cell's scratch is dropped at
  disposal rather than retained by a seal.
- `release` returns a reclaimed region's chunks to a graph-level LIFO spare list
  and `create` / `create_tree` draw from it; none of the four birth and death
  verbs changes signature. A release-then-create loop settles at zero allocator
  calls per hop, recorded as a workload in
  [observe/perf.csv](../observe/perf.csv) alongside its resident bytes.
- The spare list never holds more bumps than a proportion of a moving average
  of the live region-owning cell count, and a bump retired past that goes back
  to the allocator. Both constants enter through a `Config` at construction. A
  test unwinds a deep tree chain, runs a two-cell loop, and finds the list at or
  under the bound with the loop still drawing from it.
- Recycling is off under `cfg(miri)`, so a use-after-reclaim stays an
  allocator-visible error across the Miri slate.
- The Miri slate is clean under tree borrows, including an interior write
  through a `Cell` re-anchored at a fresh `'scratch` each step and a tenant
  write under a parked host's live borrow.
- [README.md](../README.md)'s no-frame-payload statement reads "many units of
  work, one region, at the embedder's election", and it documents the
  `'scratch` brand beside `'here`.

**Directions.**

- *Tenancy is mechanism, election is the embedder's — decided.* The substrate
  ships the tenant kind and no rule for when to use it, as
  [src/tree/README.md § What the pool does not decide](../src/tree/README.md#what-the-pool-does-not-decide)
  does for kind selection. Koan's rule is
  [the scheduler's](../../roadmap/rewrite/scheduler-on-cellgraph.md).
- *`'scratch` is a third `enter` brand, not `'severed` widened — decided.*
  `'severed` has no outlives relation to `'cell` in either direction; the
  habitat needs `'here: 'scratch`.
- *A tenant is a third handle kind — decided.* Its own handle and its own
  uncapped pool, rather than a tree-pool occupant behind a `TreeHandle`: no
  value is ever homed in a tenant, and a separate kind lets the types that name
  a value's home say so.
- *A tenant named as a host means its host — decided.* A sharing tail loop's
  first host is released while its tenants run, and the next hop has only a
  live tenant to name. The host cannot have disposed while that tenant is
  counted on it, so the resolution asks nothing of the host's own life.
- *Scratch rides a second continuation slot, not `At<'cell, 'scratch>` —
  decided.* One lifetime per family keeps `Reattachable`, `erase` and
  `reattach` as they are. An invariant `'here` structure therefore stays in the
  `'here` half and the step rejoins the halves each wake.
- *Reset at entry, never at exit — decided.* An exit reset needs `&mut` on a
  bump the step holds at `'scratch`; a parked cell keeps its scratch bytes
  until its next `enter` or its death.
- *Recycling, not reset-in-place on a live cell — decided.* Reset-in-place
  keeps the generation, so a stale tree-homed dormant — which interns no entry
  — redeems into recycled bytes; recycling's safety argument is `release`'s
  own.
- *Recycling is allocator-private and LIFO — decided.* The embedder sees no
  recycling verb, only the bound's two constants; LIFO is what makes a tail hop
  draw its predecessor's chunks in the common case.
- *The spare list is bounded against recent demand — decided.* An unbounded
  list keeps a program's peak resident for the rest of its run. The measure is
  live region-owning cells, not bumps: spares serve births, and sealed or
  absorbed storage is retention, not demand.
- *The bound's constants are a runtime `Config`, not type parameters —
  decided.* A struct cannot be a const parameter on stable Rust, and an inline
  row cannot be sized from a config trait's constant, so the config widens the
  runtime cap that already sits beside `W`.
- *Folding `W` itself into one configuration type — deferred* to a follow-up
  once a second type-level parameter wants the same treatment: the stable
  spelling is a config trait carrying the row type, which touches every public
  type that names `W`.
- *`cfg(miri)` is the cfg — decided.* It holds for every Miri run with no
  feature to pass, and both branches compile on every build.
- *Plan-visible aiming ("create this cell, preferring that dying one's chunks")
  — deferred* to a follow-up once a workload shows the LIFO list missing.
- *Scratch is a habitat, not a cell — decided.* A slab scratch cell inverts
  the crossing rule (scratch→storage is `Forced`, storage→scratch is
  `Ordinary`) and cannot redeem a tree-homed
  carrier; a scratch tree child works only as a runtime policy the embedder
  must not get wrong.
- *Both halves are re-anchored in `enter` — decided.* `enter` mints both brands
  and both writers, so the pairing of slot with brand is audited in the one
  place it is made, and the doors become field moves with no cell-kind match.
- *The scratch half takes its own family parameter, defaulting to the
  continuation family — decided.* A shared family makes the wrong half
  representable in each slot and sizes both slots to the larger half, in every
  cell and every tenant.

## Dependencies

The three mechanisms are independent of each other except at one point: a
shared region's scratch reset is a count over tenants, so that half of the
habitat lands with tenancy.

**Requires:** none — the parentless slab and the tree pool ship.

**Unblocks:**

- [Scheduler on cellgraph](../../roadmap/rewrite/scheduler-on-cellgraph.md) — elects tenancy per spawned cell and runs a fresh tail loop on recycled regions.

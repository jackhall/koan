# The DAG scheduler

What `workgraph` adds on top of the [cellgraph](cellgraph.md) substrate: a
*scheduling discipline* over cells. Cells become **nodes** with dependency edges,
a wake protocol, and terminal delivery. Everything here is defined in terms of
edges and terminals — which is exactly why none of it belongs one layer down.

This doc owns the graph's internal shape and invariants.
[scheduler-library.md](../../design/scheduler-library.md) owns the embedder-facing
consumer API (the dependence primitives, `Deps`, `Await`, the step construction
context); [reach.md](reach.md) owns what a terminal's carrier proves and who owns
its pins.

## Edges and the boundary

**Edges are first-class and are the sole boundary currency.** An edge is one
consumer→producer relationship, living in its own slab
([`edge_slab.rs`](../src/scheduler/edge_slab.rs)) — a state vector plus a
free list of recyclable indices, mirroring the node store — addressed by
`EdgeId`. An `EdgeId` is a *name*, not the edge: holding one grants the crate's
wiring and read verbs, and confers no ownership and no lifecycle duty. The
embedder wires everything through such names — parked deps, dispatch
placeholders, scope bindings, and the run's roots alike — while the edges
themselves stay in the slab. `NodeId` is a drive-loop currency: the embedder's
driver pops, steps, wires, and may do graph surgery with it (each id carries an
allocation stamp, so equality survives slot recycling), but everything deeper —
an embedder's scopes, bindings, frames — speaks edge names and slot stamps,
never node ids.

**Edge validity is self-owned.** An edge is valid until its *owner* releases
it, and an owner is always a teardown-bearing structure — the consumer node,
or the frame (a scope's, the run's) whose teardown verb carries the release.
A live edge therefore implies a live owner by construction, misuse is locally
auditable, and no edge's validity depends on remote state. Debug-only
generation stamps on slab indices make a stale `EdgeId` — a name outliving
its edge — loud.

**Every edge stores a destination-region reference — a raw pointer, not a
refcounted handle.** The destination is not always the consumer's own region (a
scope binding lands in the scope's frame region, a root in a run frame the
embedder owns), so the edge must name it; but it never needs to *own* it,
because validity is a containment guarantee rather than a detection problem:

- **The edge dies with its owner**: edge release is part of the owner's teardown
  verb, so a live edge implies a live owner.
- **The destination matches or outlives the owner**, established at wiring: the
  destination is the owner's own region, or one the owner's liveness already
  covers (its anchor's owner chain, which strong-owns strictly older ancestors
  per [reach.md](reach.md)'s DAG rule, or its region's union bundle). Both
  covers are monotone — the union bundle only absorbs and drops whole at death,
  chains never re-point — so the wiring-time fact cannot rot.

Together: destination outlives owner outlives edge, so the stored region pointer
is valid whenever the edge is, with one crate-internal deref site whose safety
argument is exactly that lattice. A dead destination is unrepresentable while an
edge lives, and the only skip in the delivery walk is a released edge. The
release-with-owner rule is soundness, not hygiene, and the embedder's half is
a pin rule with a structural witness: the install door takes the destination
as its owner (`&Rc`), so a wiring call can only name a region the caller pins
at that moment, and the destination must stay covered by the releasing owner's
liveness for the edge's life — a violation dangles rather than self-healing.
Debug builds carry a
weak shadow of the destination on each edge, asserted live at deref, so a
dangling delivery is loud while release builds stay refcount-free.

## Slots and the node-store lifecycle

Node state lives in [`NodeStore`](../src/scheduler/node_store.rs), which owns a
single `slots` vector of `SlotState` enums plus a `free_list: Vec<NodeId>` of
recyclable indices. The per-slot lifecycle:

- `PreRun(StoredWork)` — an un-run node's work in its resting form: the embedder
  builds a live `NodeWork<'a, W>`, and the install path seals its continuation
  against the slot's anchor (§ [witnessed-memory.md](witnessed-memory.md)) before
  it lands here.
- `Running` — work moved out for its step.
- `Free` — reclaimed, index on the free list.

**Slots are ephemeral.** There is no at-rest terminal state: `finalize`
distributes the terminal to every live edge (§ Delivery at finalize) and the
slot reclaims the moment its notify drains, unconditionally — no refcount, no
reclaim condition beyond that. Each index moves through `alloc_slot →
take_for_run → reinstall* → finalize`, each transition a single atomic mutator
body. A loop-shaped workload's per-iteration fanout recycles through the free
list that `alloc_slot` pulls from before extending the vectors, so scheduler
memory stays O(1) across iterations.

Because indices recycle, a `NodeId` is an index **plus an allocation stamp**,
bumped once per reclaim — reclamation being the only event that retires an
incarnation. Two ids for one index from different allocations therefore compare
unequal, so an embedder keying a decision on slot identity is safe across reuse.
The stamp is not a debug aid: it carries in release builds, because the
decisions that read it do.

Reinstallation is what makes a chain of tail-shaped continuations cost one slot
rather than one per hop: the slot's work is replaced in place and re-run, no new
node allocated. Because the reinstall applies *after* a step returns, never
mid-step, the retiring incarnation's region is past every borrow into it by the
time it is retired — the run-then-apply ordering supplies the safety, so a hop
needs no in-place region reset. The loop-carried arguments adopt into the new
incarnation inside the replace itself — terminal sources deliver at wiring —
which runs while the outcome-apply path still has the displaced anchor in hand,
so the ordering is a local variable held across the install call. A copy
verdict frees the retiring region at the replace; a pin verdict transfers it by
hold into the new incarnation's anchor bundle
([reach.md § Retention model](reach.md#retention-model)). A wiring-time
destination pointer is safe across reinstall: a slot only reinstalls after
running, which requires `pending == 0`, so it has no undelivered inbound edges,
and the new incarnation's edges are wired at replace against the new anchor.

## Push/notify dependency edges

Edges point producer → consumer. Each slot's `DepRow` carries a
`notify: Vec<EdgeId>` list of edges waiting on it; each consumer carries a
`pending: usize` counter of unfilled inbound edges. When a producer's step ends
in a terminal, the finalize walk drains its `notify` list, delivers into each
live edge's destination, decrements each consumer's `pending`, and pushes any
zero-counter consumer onto the run set. The terminal write and the notify-walk
fire in a single method body, so "every terminal fires the notify" is
type-enforced rather than restated at each call site. Consumers arrive on the
run set only when actually ready; there is no poll-and-requeue.

Every consumer wakes the same way: at pop time its pending count is zero, so
every dep has been delivered, and the deps are already ordinary residents of
the consumer's region, co-located with the continuation sealed against the same
anchor. **Step start is zero work** — no graph reads, no envelope-owning dep
slice, no per-edge wake-attribution side channel.

## The dep row and its invariants

[`DepGraph`](../src/scheduler/dep_graph.rs) stores one `rows: Vec<DepRow>`
parallel to the slot table. Each `DepRow` bundles the coordinated per-slot
fields — `notify` (forward wake edges to this slot's dependents) and `pending`
(this slot's unfilled-inbound-edge counter) — and the rows uphold three
invariants:

- **Inv-A (wake-pending coherence).** For every consumer slot `c`, `c`'s
  pending count equals the number of unfilled live edges to `c` across all
  producers' `notify` lists. Mutations go through the row, so a slot's wake
  fields cannot desync — Inv-A holds by construction.
- **Inv-B (edge containment).** An edge never outlives its owner — release
  rides the owner's teardown verb — and its destination region matches or
  outlives that owner, established at wiring (§ Edges and the boundary). This
  is the soundness of the destination deref, not bookkeeping hygiene.
- **Inv-C (lazy notify-scrub on release).** An edge's slab index is only
  recycled once every producer's walk has dropped it from every `notify` list.
  A released edge still listed is skipped by the walk (the one staleness case,
  answered by the edge slab), and the recycle path relies on Inv-C rather than
  scrubbing itself.

The rows are private and mutated only through a small surface — row
installation, the wire primitive, the finalize walk, edge release, the splice's
notify re-point — so every change preserves the per-row invariants atomically.
`Scheduler::alloc_node` orchestrates across the two sub-structs:
`NodeStore::alloc_slot` picks the index (popping the free list or extending) and
the dep graph's row installation branches privately on whether the slot is
recycled or freshly extended, writing the dep entries in lockstep.

## Work queues: two priority bands

The run set has two priority bands managed by
[`WorkQueues`](../src/scheduler/work_queues.rs). **Internal** work — notify-walk
wakeups, re-enqueues, and ready-on-arrival nodes registered in `add()` — routes
through `push_internal` / `push_internal_front` / `push_woken`. **Top-level**
submissions route through `push_top_level`, so independent top-level units execute
in submission order. The loop drains via `pop_next`, which yields internal slots
ahead of top-level ones. The routing rule (which band a push lands in) and the
priority rule (which band a pop drains first) are both enforced by the wrapper's
method surface rather than restated at each call site.

## Delivery at finalize

A terminal is `Result`-shaped: a sealed carrier, or the workload's error. The
`Result` split is the DAG layer's alone — the cell substrate has no notion of a
cell being finished forever, so it has no error type either. The producer hands
`finalize` its finished terminal as a `DeliveredTerminal` envelope — the sealed
carrier paired with its region owner and foreign pins — so a terminal's value
and the regions it reaches are one value throughout the delivery path. The
envelope is internal transit inside the finalize walk; it never crosses to the
embedder.

**Delivery happens at finalize, directly into each destination region.** The
walk visits each edge in the producer's notify list: skip it if released;
otherwise **adopt** the terminal into its destination region — the existing
`Delivered → Sealed` adopt verb (mint a description at the destination, retain
pins into the destination's union bundle), with the embedder's retention
predicate (`still_borrows`, derived on the product after the fold) deciding
deepcopy vs pin exactly as [reach.md § The library
boundary](reach.md#the-library-boundary) specifies. Then decrement the
consumer's `pending`.

The adopt itself is the workload's, through the trait's one behavioural hook:
`Workload::deliver(&terminal, dest)` receives the in-transit envelope and a
**destination operand** — a bare handle on the destination region, minted off
the slab's single deref and sealed the way a value is. The walk decides *when*
and *where*; the embedder decides what the crossing costs, and the product it
returns rests at the destination. That is the whole of the seam: no other
scheduler verb asks the embedder a question about a value. Errors deliver per-edge, cloned per destination
(`W::Error: Clone`).

**Adoption is per distinct destination, not per edge.** The walk buckets live
edges by their destination-region pointer, adopts once per distinct
destination, and fans the resulting resident out to every edge in the bucket —
a linear look-back scan over the (short) notify list, no allocation, no state
outliving the walk. An embedder parking on a placeholder therefore names the
*original* destination region on its edge, and the second write into that
region is free. Every edge names a real destination; there is no
destination-free notify-only edge kind (one that filled would be unreadable,
and its usefulness would hang on a cross-owner invariant).

**Consequences:**

- **No parked-terminal window.** The terminal is distributed the moment it
  exists. Pins live only in ordinary region channels — union bundles at rest,
  transit inside the walk — and the scheduler holds no pin holder of its own:
  retaining a node and retaining a region are different things, and slot
  lifecycle touches no pin ([reach.md § Retention model](reach.md#retention-model)).
- **Copy-verdict producers free at finalize** — the earliest possible instant.
- **Death is order-free across parties, load-bearing within an owner.** A
  consumer that dies before its producer fires releases its edges in its own
  teardown verb; the producer's later walk sees released slab entries and
  skips. No ordering is required *between* producer and consumer.
- **Speculative delivery is bounded waste.** A live consumer that never runs
  may receive a deep copy it never reads, or — on a pin verdict — hold the
  producer's region pinned until its own anchor dies. Both are bounded by the
  consumer's region lifetime, not leaks; dead edges are skipped.

**Error short-circuit** rides the same walk: a continuation never sees an
errored dep. The first errored dep short-circuits the resolve, and the
consumer's own terminal carries the error with whatever label the embedder
attached.

Top-level roots have no consumer node: each entry's root edge is owned by the
run frame it destines into, and the drain boundary — that frame's teardown —
releases it; the embedder keeps only the root `EdgeId` it reads through. Root
terminals are ordinary residents of the embedder's own regions by then, so a
drain-boundary read is a resident read, not a graph read.

## Late wiring and install

Slots reclaim at finalize, so a late edge cannot fill from a slot — but the
embedder always wires from an `EdgeId` it holds. **Install returns
filled-or-parked** — every wiring verb (`install_edge`, `install_edge_from`,
`install_deps`, and the allocators underneath them) hands back an
`InstalledEdge::{Filled, Parked}` — folding readiness probes into the install
verb:

- Wiring a new consumer to a *filled* edge **shares that edge's resident**. A
  wire-from-a-source inherits the source's destination region, so both edges
  name one region and the value is already resting in it: the share is the
  per-destination dedup the finalize walk applies, arriving structurally rather
  than as a shortcut on a general adopt. No slot involved, never a notify entry,
  and no second relocation. A consumer whose own region differs from the
  destination it inherits reads the resident there — the same read a parked
  edge's consumer takes, since a park inherits the same way.
- Wiring to an *unfilled* edge parks on that edge's producer, which is
  necessarily pre-terminal (unfilled ⇒ undelivered ⇒ slot alive) — the validity
  argument needs no discipline.

Late wiring is not a code path the embedder chooses — it is the install verb's
filled branch, taken whenever the producer finalized before the consumer's
wiring. The only discipline the filled branch needs is the ownership rule
itself: a wiring call names an edge whose owner still stands, so the edge
being read is live, and a stale name trips the slab's generation stamps.

Because the verdict rides the verb, the crate exposes **no standalone readiness
or producer-standing probe**: readiness is not a question an embedder asks, it
is what wiring answers, so no consumer can read a producer's state without
having wired the edge that makes the read sound. A `Filled` verdict is the
caller's to act on rather than something a later poll rediscovers — an errored
producer never notifies again, so a park on one would wait forever, and the
embedder propagates at once instead. The one graph question that stays
pre-wiring is `would_create_cycle` (and its edge-keyed form): parking on an
ancestor deadlocks rather than errors, so it has to be answerable before the
edge exists.

## Alias splice

The push/notify model assumes a **single producer slot per result**. A slot whose
result *is* another producer's result would otherwise become a second producer of
it. Instead the slot is **spliced out** and the producer stays sole: pre-fill, an
edge's producer pointer is the scheduler's to rewrite, so the splice re-points
the slot's parked edges at the real producer once and moves them onto its notify
list, and the slot reclaims. No aliased slot survives as a residual, no alias walk
runs on reads, and the graph logic lives in one module
([`splice.rs`](../src/scheduler/splice.rs)). If the producer is already
terminal, the spliced slot's edges take the install verb's filled branch
instead. Post-fill the resident value *is* the value — the surgery window
closes at delivery, which is the right semantics. Reinstall and relocation
surgery cannot invalidate the embedder's names for the same reason: the
embedder holds `EdgeId`s, not topology.

The wire primitive is scheduler-internal. An embedder wiring an
already-allocated slot goes through the single public door,
`Scheduler::install_deps`, which resolves the embedder's dep list — park
*source edges* plus owned producers — through it; a fresh slot's row and its
wires are initialized as one atomic step by `alloc_node` (owned deps only) and
`alloc_node_with_parks` (its sibling for a slot whose parks arrive with the
work), both routing the same primitive. One primitive, two doors — so no wiring
path can skew a row's invariants.

Every park mints the consumer its *own* slab edge off the source, inheriting the
source's destination region, and the consumer owns it under the ordinary
ownership rule (§ Edges and the boundary): the scheduler keeps no dep-side
release bookkeeping, because a consumer holds its edges only as long as it needs
their residents. A step's deps are read at step start and released right there —
the values live in the destination regions, not in the edges — and whatever
edges the consumer still owns at its terminal are released by its own teardown.

## Open work

- [Delivery at finalize](../roadmap/delivery-at-finalize.md) — the flip from
  consumer-pull to delivery in the finalize walk.
- [Delivery at replace for reinstallation](../roadmap/reinstall-delivery-at-replace.md)
  — retiring the row-level handoff hold.

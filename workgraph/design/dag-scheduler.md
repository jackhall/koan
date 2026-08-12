# The DAG scheduler

What `workgraph` adds on top of the [cellgraph](cellgraph.md) substrate: a
*scheduling discipline* over cells. Cells become **nodes** with dependency edges,
a wake protocol, terminal results, refcount reclamation, and alias splicing.
Everything here is defined in terms of edges and terminals — which is exactly why
none of it belongs one layer down.

This doc owns the graph's internal shape and invariants.
[scheduler-library.md](../../design/scheduler-library.md) owns the embedder-facing
consumer API (the dependence primitives, `Deps`, `Await`, the step construction
context); [reach.md](reach.md) owns what a terminal's carrier proves and who owns
its pins.

This doc describes the settled design. Where the in-tree code has not caught up
to a section yet, § Open work at the bottom carries the tracking item.

## Slots and the node-store lifecycle

Node state lives in [`NodeStore`](../src/scheduler/node_store.rs), which owns a
single `slots` vector of `SlotState` enums plus a `free_list: Vec<NodeId>` of
recyclable indices. One enum encodes the whole per-slot lifecycle:

- `PreRun(StoredWork)` — an un-run node's work in its resting form: the embedder
  builds a live `NodeWork<'a, W>`, and the install path seals its continuation
  against the slot's anchor (§ [witnessed-memory.md](witnessed-memory.md)) before
  it lands here.
- `Running` — work moved out for its step.
- `Done(Result)` — the terminal: a sealed carrier, or the workload's error.
- `Aliased(NodeId)` — spliced out to its producer (§ Alias splice).
- `Free` — reclaimed, index on the free list.

Each index moves through `alloc_slot → take_for_run → reinstall* → finalize →
free_one`. Each transition is a single atomic mutator body, so the
recycle-vs-extend choice, the take/reinstall pairing, the terminal write, and
reclamation are each encapsulated. Because work and result occupy the *same* enum
slot, no call site outside `NodeStore` can land a `Done` without the node having
been taken, nor read a result before it is `Done`.

Reinstallation is what makes a chain of tail-shaped continuations cost one slot
rather than one per hop: the slot's work is replaced in place and re-run, no new
node allocated, and the retiring incarnation's region turns over under retention
(§ Terminals and retention). Because the reinstall applies *after* a step returns,
never mid-step, the retiring incarnation's region is past every borrow into it by
the time it is retired — the run-then-apply ordering supplies the safety, so a hop
needs no in-place region reset.

## Push/notify dependency edges

Edges point producer → consumer. Each slot's `DepRow` carries a
`notify: Vec<NodeId>` list of dependents waiting on it; each consumer carries a
`pending: usize` counter of unresolved deps. When a slot writes a terminal, the
notify-walk drains its `notify` list, decrements each consumer's `pending`, and
pushes any zero-counter consumer onto the run set. The terminal write and the
notify-walk fire in a single method body that pairs `NodeStore::finalize` with
`DepGraph::drain_notify`, so "every terminal write fires the notify" is
type-enforced rather than restated at each call site. Consumers arrive on the run
set only when actually ready; there is no poll-and-requeue.

Every consumer wakes the same way: at pop time its pending count is zero, so every
dep is terminal, and the step reads each resolved dep off the view by index and
hands the results to the slot's continuation. There is no per-edge wake-attribution
side channel — a continuation that re-resolves reads its producers itself, not a
"who woke me" list. `DepGraph::drain_notify` returns the per-consumer `hit_zero`
flag so the enqueue-on-zero runs off a single drain.

A dep edge is a **wire**, and there is one kind. A wire does two jobs. While
the producer is pending, it is the wake edge above. And from the moment it is
installed until its consumer releases it, it is one **standing destination** of
the producer — an entry in the producer's standing-destination count, the
refcount that decides when the producer is reclaimed (§ Refcount reclamation)
and when its retention hold releases (§ Terminals and retention). The scheduler
records no ownership distinction between deps — who outlives whom falls out of
who still holds a wire, not out of an edge tag.

## The dep row and its invariants

[`DepGraph`](../src/scheduler/dep_graph.rs) stores one `rows: Vec<DepRow>`
parallel to the slot table. Each `DepRow` bundles the coordinated per-slot
fields — `notify` (forward wake edges to this slot's dependents), `pending`
(this slot's unresolved-dep counter), the slot's backward wires to the producers
it depends on, and its own standing-destination count — and the rows uphold
three invariants:

- **Inv-A (wake-pending coherence).** For every consumer slot `c`,
  `rows[c].pending == |{ p : c appears in rows[p].notify }|`. Mutations go through
  the row, so a slot's wake fields cannot desync — Inv-A holds by construction.
- **Inv-B (destination coherence).** For every producer slot `p`, its
  standing-destination count equals the number of installed-and-unreleased wires
  to `p` (alias-resolved) plus any root destination the run holds on it
  (§ Refcount reclamation). Every wire increments the count at installation,
  unconditionally — no wiring path branches on producer readiness for
  accounting — and each is released exactly once, at its consumer's end of step
  or at that consumer's death, both of which run the same wire release.
- **Inv-C (lazy notify-scrub on free).** A slot `c` is only freed once every
  producer's `drain_notify` has run and removed `c` from every `rows[*].notify`.
  The free path relies on Inv-A and Inv-C still holding rather than scrubbing
  itself.

Inv-B is what makes reclamation a local decision: a slot's count reaching zero
*is* the proof that no consumer, root, or forwarding alias can still read it, so
the reclaim needs no edge tags and no knowledge of who allocated what.

The rows are private and mutated only through a small surface — row
installation, the wire primitive, `drain_notify`, the wire release,
`splice_notify` — so every change preserves the per-row invariants atomically.
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

## Alias splice

The push/notify model assumes a **single producer slot per result**. A slot whose
result *is* another producer's result would otherwise become a second producer of
it. Instead the slot is **spliced out** as an alias, and the producer stays sole.
All the graph logic lives in [`splice.rs`](../src/scheduler/splice.rs):

- If the producer is already terminal, the slot finalizes directly with the
  producer's terminal.
- Otherwise the slot's step yields an alias step, and the loop calls
  `Scheduler::splice_forward`: consumers already parked on the slot move onto the
  producer's notify list (`DepGraph::splice_notify`), and the slot's `SlotState`
  becomes `Aliased(producer)`. The aliased slot never fires; the producer's fire
  wakes the moved consumers directly.

Reads follow the alias to the real producer: `Scheduler::resolve_alias` walks the
chain (iterative, always pointing downstream to a real producer, so it terminates
and never cycles), and the result reads resolve through it. Wiring resolves it
too — the wire primitive wires a late consumer against the *resolved* producer.
An already-finalized producer contributes no wake bookkeeping (no notify entry,
no pending increment — the consumer never parks on a slot that will not fire),
but its wire still counts as a standing destination, so the consumer's read is
covered by the producer's retention hold (Inv-B). Neither the store nor the dep
graph has to be alias-aware on its own; the alias contract lives in one module.

The wire primitive is scheduler-internal. An embedder wiring an
already-allocated slot goes through the single public door,
`Scheduler::install_edges`, which routes a `ResolvedDeps` list through it; a
fresh slot's row and its wires are initialized as one atomic step by
`alloc_node`, which routes the same primitive. One primitive, two doors — so no
wiring path can skew the destination count.

## Refcount reclamation

A reinstalled slot is reused, but the work it spawns each iteration is not: every
sub-unit is a fresh slot the parent wires as a dep. Without reclamation those
slots accumulate per iteration, so a loop-shaped workload costs O(n) scheduler
memory even when its data footprint is O(1).

Reclamation is refcount-driven: a slot is reclaimed exactly when its
standing-destination count decrements to zero. There is one verb, the **wire
release**: a consumer runs it once at its end of step, and it also runs when a
consumer dies on an error path — the two cases are the same code. The wire
release drops every wire the consumer holds; each drop decrements that
producer's count, and a producer that hits zero (and is not mid-run, via
`NodeStore::is_live`) is reclaimed on the spot — retention hold released, anchor
dropped, slot recycled onto the free list, and its own wires released
recursively. The recursion stops wherever a count stays positive: a producer
another consumer still wires, or one the run still holds a root destination on,
survives — reclaiming one consumer can never reach into a shared producer's
subtree, and no path force-kills a still-counted slot. The success-path release
runs after a step's continuation returns its outcome and *before* the outcome is
applied, so freed indices are on the free list before the step's follow-on work
is submitted.

The net effect: a loop whose only persistent state is its carried result runs in
O(1) scheduler memory across iterations, with the per-iteration fanout recycled
through the free list that `alloc_slot` pulls from before extending the vectors.

Top-level roots have no consumer to wire them, so they get their standing
destination another way: the **run itself** holds one destination on each entry
slot — its **root destination** — which the embedder releases at the drain
boundary. Root terminals therefore stay readable until the embedder is done
with the run, and each submission's persistent slots are reclaimed at drain
rather than accumulating past it.

## Terminals and retention

A terminal is `Result`-shaped: a sealed carrier, or the workload's error. The
`Result` split is the DAG layer's alone — the cell substrate has no notion of a
cell being finished forever, so it has no error type either.

Delivery is envelope-only, in **both** directions. A dep whose value must outlive
the resolving step travels as its [`Delivered`](../src/witnessed/delivered.rs)
envelope — the sealed carrier paired with the producer's retained region owner and
the value's foreign pins — so the value stays in its producer's region and the
consumer adopts it at its own step brand. No dep crosses to a continuation as a
bare pin or a pre-relocated value, including on the error-catch channel. The
producer side is the same currency: `finalize` and `rehome_terminal` take the
workload's finished terminal as a `DeliveredTerminal` envelope, never a carrier
plus a separately-passed reach. A terminal's value and the regions it reaches are
therefore one value throughout — no signature in the terminal path can be handed a
coverage belonging to some other terminal, and none can be handed a value with its
coverage dropped.

Retention is destination-driven and reach-independent: `finalize` materializes
the producer's `{ owner, reach }` hold under the slot's already-standing
destination count — the wires installed since its birth plus any root
destination the run holds — and both halves release when that count decrements
to zero (§ Refcount reclamation). There is no seed arithmetic at finalize and no separate late-wire
channel: a destination stands from wiring to release, so a wired consumer's
`dep_delivered` can never observe a released hold — the read verb's hold lookup
is total. Release is a function of standing destinations only — never of any
value's reach ([reach.md § Retention model](reach.md#retention-model)).
The hold's `reach` half is derived inside `finalize` from the terminal envelope's
own coverage with its residence released — the hold owns that region as its `owner`
field, so re-listing it there would be a second `Rc` on the very frame the hold's
release frees, and a tail loop's retiring region would never turn over.

**Error short-circuit** is built into the same walk: a continuation never sees an
errored dep. The first errored dep short-circuits the resolve, and the consumer's
own terminal carries the error with whatever label the embedder attached to the
envelope.

## Open work

- [Wire-refcounted retention and reclamation](../roadmap/wire-refcount-retention.md)
  — implements the single-wire accounting above (unconditional counting, the
  wire release, refcount reclamation, root destinations). Until it ships, the
  in-tree dep graph still carries the split accounting this design replaces.

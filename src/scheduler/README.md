# The scheduler

The deferred-work drain koan runs on, built directly over
[`cellgraph`](../../cellgraph/README.md)'s cells and liveness matrix, reached
only through [`memory`](../memory/README.md). It sits above
[`values`](../values/README.md) and [`function`](../function/README.md), and
neither of them names it.

A unit of work **is** a cell. Its region, its erased continuation and its holds
are the cell's; this module adds the submission table, the work queue, the drain
protocol and delivery, and nothing else. Liveness is the matrix's — no reference
count, no pin bundle and no antichain fold lives here, and a cell is reclaimed
the instant no hold names it.

## The drain

`Scheduler` holds a `CellGraph` over three koan types — the continuation family,
its scratch half, and the delivery bundle — beside two queues and a buffer of
requests. Every field is that scheduler's own: no static, no thread-local and no
lazily minted cell, so a second scheduler runs beside the first with nothing
shared but the program text and the shapes in it.

The loop pops a cell, enters it, runs the step its continuation names, and acts
on what the step returns:

- `Done` — the step is finished and has already filled its consumer's receipt,
  so nothing is in flight when the drain releases the cell.
- `Wakes` — the same, and its delivery filled the last slot of the named
  consumer's run: the drain releases this cell and queues that consumer. This is
  the only way a parked cell wakes.
- `Park` — the step registered a receipt run and described its children; the
  drain creates them and leaves the cell alone until the run completes.
- `Tail` — the drain creates the successor, then releases this cell, in that
  order.
- `Failed` — the drain abandons the run.

Two queues, `in_flight` strictly ahead of `fresh`: an in-progress computation
finishes before a new unit starts, and a tail successor pushed to the front of
`in_flight` runs before any sibling work.

The queue emptying with the graph empty beside it is success. Anything else is
`DrainStalled`, the only error the scheduler defines. A koan error is a
[tagged value](../values/README.md#what-a-value-is) and travels between cells as
data, so no `Result` passes from one cell to another and a consumer checks the
results it redeems.

## What a step may name

`enter` quantifies a step's three brands — `'step`, `'here` and `'scratch` — per
call, so a step's return type can name none of them. What crosses back out of a
step is a handle, an index, a dormant carrier or a borrow of program storage.
That is why `Action` carries no region borrow, and why a step describes its
children by pushing `Request`s into a drain-owned `Spawns` buffer it is handed
by `&mut`: a slice of requests would have nowhere to be branded.

A `Request` names the work — a placement, a step, a birth state and a slot — and
not the place. The drain fills in the child's `Provenance` from the cell the
request was pushed in and the slot it names, so a child is always born under the
cell that asked for it and always reports to that cell's run; no step can name a
destination that is not its spawner's.

A step also cannot create or release a cell. `StepContext` has no `create`, no
`release` and no second `enter`, so every birth and every death is the drain's,
performed from the `Action` the step handed back.

## The continuation

A cell's continuation comes in two halves, each its own family, so the wrong half
is unrepresentable in each slot. `ContinuationFamily` re-anchors at the executing
cell's region brand and is what the drain reads to run a step; `ScratchFamily`
re-anchors at the scratch habitat's brand and carries a parked cell's in-progress
state across a park.

`Continuation` has one arm here, `Native`: a `NativeStep` function pointer, the
cell's `Provenance`, and the `State` the step runs over. The pointer is
higher-ranked over the three step brands and not over `'graph`, so one pointer
runs in any cell at any step while the state it reads, the children it describes
and the action it returns all name the graph they belong to. The rewrite's second
layer adds a component arm beside `Native`, and nothing else.

A *birth* continuation is the family at `'cell = 'graph`, because `create`,
`create_tree` and `create_tenant` all take one there: its state holds only words,
program storage and dormant carriers. A continuation a step stores for itself
through `store_successor` is at `'here` and may hold region borrows freely.

`Provenance` is what a cell carries about its place in the graph, for the drain's
use: the parent a sibling is born under or the host a co-tenant is born of, and
the `Destination` — consumer handle and slot — its result goes to. Every field is brand-free, so it
survives a tail hop verbatim — which is what "the successor inherits its receipt"
means. `cellgraph` exposes no parent accessor, so a cell remembers its place here
rather than in a table beside the graph.

## The two ways a cell waits

They never overlap.

A **binder dependency** is waited on by a unit that has no cell yet. A body's
reference graph is known before it runs, so a unit is submitted with a count of
unmet dependencies, the count is decremented as each producing unit finishes, and
the drain creates the cell when it reaches zero. A reader therefore never
observes a binding whose binder has not run — the discipline
[scopes](../scope/README.md#placeholders-and-writes) already assert, where a read
that finds an empty slot is a scheduler bug. This is why there is no wait record,
no waiter list and no per-slot park, and why the drain holds a record per
*unsubmitted* unit rather than per parked cell.

A **sub-dispatch** is waited on by a live, parked cell, and the count lives in
the substrate's own receipt run. A producer's delivery door decrements it, and
the consumer wakes once, when the last slot fills.

## Placement

One bit picks a spawned cell's habitat, supplied by the spawner per spawn.

| `Placement` | door | the child's region | the spawner's price |
|---|---|---|---|
| `Fresh` | `create_tree` | its own, reclaimed whole at death | a result placed operand-free crosses free |
| `Shares` | `create_tenant` | none — the spawner's, at the spawner's own brand | an embedded argument crosses free, because nothing crosses |

The same bit decides a tail hop. A `Fresh` hop draws a recycled region and the
loop runs in memory independent of hop count; a `Shares` hop joins its
predecessor's region, whose growth the result retains anyway. Bounded memory has
a price the bit also carries: sibling tree cells are `Apart`, so a `Fresh` hop's
arguments homed in its predecessor cross by a forced copy, every hop, while an
argument homed in the caller's frame or above is `Under` and free. A loop that
threads a large state it built itself wants `Shares` whatever its return type
says.

The hint is never a contract. A wrong `Fresh` costs a priced copy, a wrong
`Shares` costs delayed reclaim, and soundness rests on the substrate's brands
either way. Where the bit comes from for a koan function is the top level's, not
this module's.

A tenant's scratch is its host's, so a tenant writes its intermediates there
while a result bound for storage is built in host storage from the start.

## Delivery

A producer pushes. It builds its result in its consumer, fills its receipt slot
through `cellgraph`'s
[delivery doors](../../cellgraph/README.md#passing-values-between-cells), and
dies.

Which door depends on where the result is bound:

- **Bound for the consumer's storage** — one the consumer embeds, binds or passes
  on. The producer builds it in the consumer's region from the start with
  `alloc_into`, `keep`s it, and files the dormant carrier with
  `deliver_carrier`. Never through scratch first: a value that passes through
  scratch comes back at `'scratch` and can never be embedded in storage again.
  The carrier has no region brand, so it rests in a scratch slot like any other.
- **Fresh and only read** — a condition, a computed lookup key, a discarded
  statement value. The producer fills the consumer's scratch habitat with
  `deliver_scratch`, whose build takes no operands and is quantified over the
  consumer's own brand. A scratch result is not short-lived by nature: it lasts
  for as long as the consumer's scratch continuation names it, and never past the
  cell.

Because the scratch build takes no operands, a read-only result that borrows data
already in the consumer goes as a carrier too.

No envelope and no mailbox holds a value at rest outside a cell. The outstanding
count lives in the receipt run, each producer carries its consumer and slot in
its `Provenance`, and a tail hop hands that pair to its successor.

## Tail hops

A tail call is a create plus a release, in that order. The successor is born a
sibling of its predecessor or a co-tenant of its host, and inherits its receipt;
the predecessor is released once the successor holds every argument that was
homed in it. The hand-off:

1. The predecessor's step `keep`s every argument the successor needs, getting
   dormant carriers, and puts them in the successor's birth continuation.
2. It returns `Action::Tail`, carrying its `Provenance.receipt` forward
   unchanged.
3. The drain creates the successor — `create_tree` under the predecessor's own
   parent for `Fresh`, `create_tenant` on the same host for `Shares`.
4. The successor's first step redeems each dormant carrier, entitled by root
   identity, and copies it in through `alloc_here`.
5. Only then does the drain release the predecessor.

A tail hop is a tree cell or a tenant, never a slab cell. A slab successor would
have to `hold` its predecessor to redeem, which pins the predecessor's region
into its row, so the release would seal rather than reclaim and constant
occupancy would be lost. Tree siblings share a root, and root identity is the
entitlement — no hold, no pin, no seal.

## Memory

Slab cells are rare: the tree pool takes no cap, so a call subtree deeper than
any slab cap runs on tree cells without touching the matrix, and the top level's
statements are tree children of one storage-only root. The matrix width is
therefore one word — `WIDTH` in [substrate.rs](../memory/substrate.rs) is `1`,
sixty-four slab slots.

## The import rule

Outside doc comments and `#[cfg(test)]` this module names `crate::function`,
`crate::memory` and `crate::values`, and nothing else in the crate. It does not
name `scope`, `parse` or `elaborate`. `cellgraph` is reached only through
`memory`, and `cellgraph` itself depends on neither this module nor koan.
`tests/boundary.rs` reads the source to hold the rule there; the work queue and
the request buffer are the two owning heap types it blanks, because each is the
scheduler's own runtime state and never a value in a region.

## Testing

The tests drive the drain with native-step workloads and no dispatch layer
present: a call, a tail loop, a subtree deeper than any slab cap, a diamond of
submissions, and a consumer parked on several producers. `tests/continuation.rs`
holds the round trip a continuation makes between `'graph` and a step's `'here`;
`tests/drain.rs` holds the loop itself, including two schedulers running beside
each other and sharing nothing; `tests/calls.rs` holds a call at each placement
and `tests/delivery.rs` a consumer parked on three producers. A native step is a
bare `fn` and carries no closure state, so what a step observes it records in
`tests/native.rs` for the test around it to read back.

## Open work

- [The top level on the scheduler](../../roadmap/rewrite/top-level-on-the-scheduler.md)
  — what turns a koan program into work for this drain.

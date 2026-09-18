# Scheduler on cellgraph

The rewrite's bottom layer: the deferred-work drain Koan runs on, built directly
over `cellgraph`'s cells and liveness matrix. What turns a koan program into work
for it is [the top level on the scheduler](top-level-on-the-scheduler.md).

**Problem.** Koan's scheduler is `workgraph`'s DAG layer
([dag-scheduler.md](../../workgraph/old_design/dag-scheduler.md)), and that layer
sits on `workgraph`'s own cell substrate — the witnessed module with its
reference-counted pin bundles and antichain fold, and a node store
([node_store.rs](../../workgraph/src/scheduler/node_store.rs)) that interleaves
cell state with dep edges, park/notify bookkeeping, terminal delivery and
splicing. `cellgraph` ([cellgraph/README.md](../../cellgraph/README.md))
supplies the cell half with matrix liveness, tree cells, tenants, receipt runs
and delivery doors, and nothing above it uses it: the substrate the design
settled on has no scheduler, and the scheduler Koan runs on is the substrate the
design moved off. The old runtime's execute layer
([execute.rs](../../src/machine/execute.rs)) was shaped around that scheduler's
envelopes — `Delivered`, the finalize walk's carried reach, the per-node
`WorkingExpression` — so its step protocol is not a contract the rewrite can
restate; it is a record of what the old substrate required.

**Acceptance criteria.**

- The scheduler is a koan module that reaches `cellgraph` only through
  [`memory`](../../src/memory/README.md) and sits above `values` and
  `function`; neither names it, `cellgraph` depends on neither it nor `koan`,
  and a boundary test reads the module's source and holds the import rule.
- A unit of work is a `cellgraph` cell: its region, its erased continuation and
  its holds are the cell's, and the scheduler adds only the submission table,
  the work queue, the drain protocol and delivery.
- Liveness is the matrix's: no reference count, pin bundle or antichain fold
  exists in the scheduler, and a cell is reclaimed the instant no hold names it.
- A step hands the drain an `Action` naming nothing but `'graph` — a handle, an
  index, a dormant carrier or program storage — and the drain performs every
  create and every release. No step creates or releases a cell.
- A unit whose dependencies are unmet has no cell: the drain holds it as a
  submission with a count, decrements that count as each producing unit
  finishes, and creates the cell when it reaches zero. So the only thing a live
  cell ever waits on is its receipt run, and the drain holds a record per
  unsubmitted unit rather than per parked cell.
- A producer builds its result in its consumer and is released at `Done`: a
  result bound for the consumer's storage — one it embeds, binds or passes on —
  lands in the consumer's region and its receipt is the dormant carrier, and a
  fresh result the consumer only reads lands in the consumer's scratch habitat,
  its receipt is the value, and it lasts for as long as the consumer's scratch
  names it. Each fills a slot of the receipt run the consumer registered when it
  parked, and a consumer parked on several producers wakes once, when the last
  slot fills. No envelope and no mailbox holds a value at rest outside a cell.
- A tail call is a create plus a release, in that order: the successor is born a
  sibling of its predecessor, or a co-tenant of its host, and inherits its
  receipt — its consumer and slot — and the predecessor is released once the
  successor holds every argument that was homed in it. A tail hop is a tree cell
  or a tenant, never a slab cell. A deep self-recursive loop runs at constant
  slab occupancy and at no more than two live hops.
- A call subtree deeper than the slab cap runs on tree cells without touching
  the matrix.
- A one-bit placement hint picks a spawned cell's habitat: a *fresh* spawn runs
  in a tree child with its own region and places its result operand-free, a
  *shares* spawn runs in a tenant of the spawner's frame and embeds an argument
  with no copy, and a *fresh* tail loop runs in memory independent of hop count
  where a *shares* one joins its predecessor's region. Inverting the bit over the
  same workload changes what is copied and what is retained and no value the
  workload computes.
- The liveness matrix is one word wide: `WIDTH` in
  [substrate.rs](../../src/memory/substrate.rs) is `1`, 64 slab slots.
- The scheduler's state — the work queue, the submission table and the parked
  count — is a graph's own, held in no static and no thread-local, so a second
  graph runs beside the first with nothing shared but the program text and its
  shapes.
- The scheduler's tests drive the drain with native-step workloads — a call, a
  tail loop, a subtree deeper than the cap, a diamond of submissions, and a
  consumer parked on several producers — with no dispatch layer present, and its
  Miri slate is clean.
- The `workgraph` crate is gone from the workspace; its `old_design/` and
  `old_roadmap/` trees remain as the requirements record.

**Directions.**

- *A koan module, not a crate and not `workgraph` rebuilt — decided.* The drain
  applies koan's `Action` directly: no workload trait and no verdict lowering
  between the scheduler and the step. Of `workgraph`
  ([adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md)) only
  the drain loop and its two-queue discipline are carried over, by hand; the
  edge slab, the node store and alias splicing have no job once a destination is
  a cell handle.
- *Where the step protocol lives — decided.* A step cannot create a cell, so it
  hands the drain an `Action` as data — finish, park on the children it
  described, tail-call — and the drain does every create and release. The
  continuation is a koan enum whose one variant here is a native step: a
  function pointer over a state value, the shape a builtin body takes. The
  component step is [the top level's](top-level-on-the-scheduler.md).
- *An `Action` carries no region borrow — decided.* `enter` quantifies `'step`,
  `'here` and `'scratch` per call, so a step's return type can name none of
  them. Everything crossing back out is a handle, an index, a `Dormant` or a
  `'graph` borrow — and the same constraint settles the tail hop below, since a
  birth continuation is taken at `'graph` too. A step describes children by
  pushing them into a drain-owned buffer it is handed by `&mut`, because a slice
  of requests would have nowhere to be branded.
- *Binder dependencies are wired before submission, never parked on — decided.*
  A body's reference graph is known before it runs, so a unit is submitted with
  a count of unmet dependencies and given a cell only when that count reaches
  zero. A reader therefore never observes a binding whose binder has not run —
  the discipline [scopes](../../src/scope/README.md#placeholders-and-writes)
  already assert, where a read that finds an empty slot is a scheduler bug. This
  is why the scheduler needs no wait record, no waiter list and no per-slot
  park: the two things a unit can be waiting for are its dependencies, which it
  waits for without a cell, and its receipt run, which it waits for as a live
  parked cell.
- *Tree cells versus slab cells — decided.* Per
  [cellgraph/src/tree/README.md](../../cellgraph/src/tree/README.md): a call
  subtree is a stack discipline and takes tree cells; a tenant is for work whose
  results share structure with its host's. Slab cells are rare enough that the
  matrix width drops to one word, 64 slots.
- *Errors — decided.* A lowered error is a tagged value
  ([src/values/README.md](../../src/values/README.md#what-a-value-is)), so no
  `Result` passes between cells and a consumer checks the results it redeems.
  The drain's own failure — the queue empty with units unsubmitted or cells
  still live — is the only error the scheduler defines.
- *Delivery — decided.* A producer pushes: it builds its result in its
  consumer — in the consumer's region when the result is bound for storage, in
  the consumer's scratch habitat when it is built fresh and only read, which is
  what a condition, a computed lookup key or a discarded statement value is —
  fills its receipt slot through `cellgraph`'s delivery doors
  ([cellgraph/src/receipt.rs](../../cellgraph/src/receipt.rs)), and dies. A
  result bound for storage is built in the consumer's region from the start,
  since a value that passes through scratch comes back at `'scratch` and can
  never be embedded in storage again; its receipt is the dormant carrier, which
  has no region brand and so rests in a scratch slot like any other. The scratch
  fill takes no operands, so a read-only result that borrows data already in the
  consumer goes as a carrier too. A scratch result is not short-lived by nature:
  it lasts across steps for as long as the consumer's scratch continuation names
  it, and never past the cell. The outstanding count lives in the receipt run,
  each producer carries its consumer and slot index, and a tail hop hands that
  pair to its successor.
- *Own region versus the caller's region for a sub-dispatch — decided.*
  `cellgraph` ships a tenant cell — one with no region of its own, whose writer
  is its host's — and the scheduler elects it per spawned cell from a one-bit
  hint. A fresh result is built in a tree child with its own region and placed
  operand-free into its destination, so the child reclaims whole at death; a
  sharing result is built by a tenant of the caller's frame, which embeds its
  arguments at no price. The same bit decides a tail hop: a fresh hop draws a
  recycled region and the loop runs in bounded memory, a sharing hop joins its
  predecessor's region, whose growth the result retains anyway. Bounded memory
  has a price the bit also carries: sibling tree cells are `Apart`, so a fresh
  hop's arguments homed in its predecessor cross by a forced copy, every hop,
  while an argument homed in the caller's frame or above is `Under` and free. A
  loop that threads a large state it built itself wants *shares* whatever its
  return type says. The hint is never a contract — a wrong *fresh* costs a
  priced copy, a wrong *shares* costs delayed reclaim, and soundness rests on
  the substrate's brands either way. This item ships the bit as a mechanism the
  workload supplies per spawn; where the bit comes from for a koan function is
  [the top level's](top-level-on-the-scheduler.md).
- *A tenant's transients — decided.* A tenant's scratch is its host's, so a
  tenant writes its intermediates there, while a result bound for storage is
  built in host storage from the start. The bump goes back at a step end that
  finds nothing at rest naming it, which a consumer parked in the same frame
  usually prevents, so a frame's scratch often lives as long as its storage
  does. What the habitat guarantees regardless is that scratch is never pinned,
  spliced or absorbed: a sharing sub-dispatch's result may be retained past its
  frame by a pin, and the working memory it was built from is freed no later
  than the frame's retirement. Carried state a hop keeps rides its scratch
  continuation, or host storage when the result embeds it.
- *How a tail hop's arguments reach the successor — decided.* The predecessor's
  step puts them to rest with `keep`, the drain creates the successor with the
  dormant carriers in its birth continuation, and the successor's first step
  redeems them — entitled by root identity, which covers a sibling — and copies
  them in through `alloc_here`, before the drain releases the predecessor. The
  alternative, the predecessor placing them with `alloc_into`, costs it a second
  step after the successor's birth. Root identity is also why a tail hop is
  never a slab cell: a slab successor would have to `hold` its predecessor to
  redeem, which pins the predecessor's region into its row, so the release would
  seal rather than reclaim and constant occupancy would be lost.
- *Threads — decided: not designed here, not precluded.* The unit would be the
  top-level component, one graph per thread: `cellgraph` is not `Send`, and a
  step holds the region table shared while every death verb takes it
  exclusively, so threads in one graph would serialize on every release. What
  that needs and this item does not build is a top-level slot array that is
  `Sync`, a binding readable from a graph other than the one whose root holds
  it, and a wake that crosses graphs, since a bind is a delivery and a delivery
  reaches one graph's cells. What this item owes it is per-graph state and a
  call subtree that stays in the graph that admitted it.

## Dependencies

[Values](../../src/values/README.md), what a cell delivers, and cellgraph's
[tenant cells, scratch habitat, receipt runs and recycled
regions](../../cellgraph/README.md#the-cell) already ship, as do the [delivery
doors](../../cellgraph/README.md#passing-values-between-cells) a producer fills
its consumer's receipt through. This item subsumes `workgraph`'s own
[adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md), which stays
as a requirements record for the module.

**Requires:** none — the substrate it stands on ships.

**Unblocks:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — a koan
  program needs the drain that runs it.

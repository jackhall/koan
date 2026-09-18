# Scheduler on cellgraph

The rewrite's bottom layer: the deferred-work scheduler Koan drives, built
directly over `cellgraph`'s cells and liveness matrix.

**Problem.** Koan's scheduler is `workgraph`'s DAG layer
([dag-scheduler.md](../../workgraph/old_design/dag-scheduler.md)), and that layer
sits on `workgraph`'s own cell substrate — the witnessed module with its
reference-counted pin bundles and antichain fold, and a node store
([node_store.rs](../../workgraph/src/scheduler/node_store.rs)) that interleaves
cell state with dep edges, park/notify bookkeeping, terminal delivery and
splicing. `cellgraph` ([cellgraph/README.md](../../cellgraph/README.md))
supplies the cell half with matrix liveness and tree cells, and nothing above it
uses it: the substrate the design settled on has no scheduler, and the scheduler
Koan runs on is the substrate the design moved off. The old runtime's execute
layer ([execute.rs](../../src/machine/execute.rs)) was shaped around that
scheduler's envelopes — `Delivered`, the finalize walk's carried reach, the
per-node `WorkingExpression` — so its step protocol is not a contract the
rewrite can restate; it is a record of what the old substrate required.

**Acceptance criteria.**

- The scheduler is a koan module that reaches `cellgraph` only through
  [`memory`](../../src/memory/README.md) and sits above `parse`, `values`,
  `scope` and `function`; none of those names it, `cellgraph` depends on
  neither it nor `koan`, and a boundary test reads the module's source and
  holds the import rule.
- A unit of work is a `cellgraph` cell: its region, its erased continuation and
  its holds are the cell's, and the scheduler adds only dep edges, the work
  queue, the drain protocol and delivery.
- Liveness is the matrix's: no reference count, pin bundle or antichain fold
  exists in the scheduler, and a cell is reclaimed the instant no hold names it.
- A producer builds its result in its consumer and is released at `Done`: a
  result bound for the consumer's storage — one it embeds, binds or passes on —
  lands in the consumer's region and its receipt is the dormant carrier, and a
  fresh result the consumer only reads lands in the consumer's scratch habitat,
  its receipt is the value, and it lasts for as long as the consumer's scratch
  names it. Each fills a slot of the receipt run the consumer registered when it
  parked, a consumer parked on several producers wakes once — when the last slot
  fills — and the drain holds no per-parked-cell record. No envelope and no
  mailbox holds a value at rest outside a cell.
- A park names the frame whose activation holds the awaited slots, never a
  producer cell: the awaiter writes its wait record into that frame's region,
  and the cell that binds a slot reads the record and fills each awaiter's
  receipt with a token the awaiter answers by re-reading the slot. A binder
  reached by a tail call, whose predecessor's handle is gone, wakes its awaiters
  as its predecessor would have, and a reader parks on a binder whose own cell
  is created after the park.
- A top-level binding lives in the region of a **root** slab cell that is
  created before the first component, released after the last, and never
  entered. The top level's slot array lives in program storage at `'graph` and
  a bound slot holds a dormant carrier homed in the root, so a statement cell
  claims, binds and reads a slot with no door; a binder builds its value in the
  root's region with what the value reaches pinned, and a reader holds the root
  once and redeems. A binder's region seals at retirement when the root holds
  it and is reclaimed when it does not, its intermediates go to its scratch
  habitat and are freed at retirement either way, and nothing that reaches a
  top-level bind is copied by anything but the verdict.
- A tail call is a create plus a release, in that order: the successor is born
  a sibling of its predecessor and inherits its receipt — its consumer and
  slot — and the predecessor is released once the successor holds every argument
  that was homed in it. A deep self-recursive loop runs at constant slab
  occupancy and at no more than two live hops.
- A call subtree deeper than the slab cap runs on tree cells without touching
  the matrix.
- The top level's components are slab cells admitted in position order — a
  component's position being its lowest member's — from a marker into the
  top-level AST: a refused create leaves the marker where it is, the drain
  resumes admission after a release, and a program with more top-level
  components than the slab cap runs to completion at any cap that admits one
  component beside the root.
- A deferred-only component of a body's bindings
  ([src/scope/README.md](../../src/scope/README.md#visibility)) is one unit of
  work: one cell claims every member's slot at submission, a refused tie
  ([src/function/README.md](../../src/function/README.md#the-tie)) naming a
  pending binder parks on that binder's slot, a refused tie naming an eager part
  of a data member becomes a sub-dispatch whose value the step supplies to the
  tie by site when it re-runs, a step refused on both kinds at once wakes once
  when the last of them fills, and the one tie that succeeds binds every
  member's slot from the knot it hands back.
- The per-function hint picks a spawned cell's placement: a *fresh* function
  runs in a tree child with its own region and places its result operand-free,
  a *shares* function runs in a tenant of the caller's frame and embeds an
  argument with no copy, and a *fresh* tail loop runs in memory independent of
  hop count where a *shares* one joins its predecessor's region. Inverting
  either hint over the same workload changes what is copied and what is
  retained and no value the program computes.
- The liveness matrix is one word wide: `WIDTH` in
  [substrate.rs](../../src/memory/substrate.rs) is `1`, 64 slab slots.
- The scheduler's state — the work queue, the parked count, the slab cap and
  the admission marker — is a graph's own, held in no static and no
  thread-local, so a second graph on a second thread runs beside the first with
  nothing shared but the program text and its shapes.
- The scheduler's tests drive the drain with native-step workloads — calls, a
  tail loop, a subtree deeper than the cap, a component with a pending binder
  and an eager part — with no dispatch layer present, and its Miri slate is
  clean.

**Directions.**

- *A koan module, not a crate and not `workgraph` rebuilt — decided.* The drain
  applies koan's `Action` directly: no workload trait and no verdict lowering
  between the scheduler and the step. Of `workgraph`
  ([adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md)) only
  the drain protocol is carried over, by hand; the edge slab, the node store
  and alias splicing have no job once a destination is a cell handle. The
  `workgraph` crate is deleted when this item retires.
- *Where the step protocol lives — decided.* A step cannot create a cell, so it
  hands the drain an `Action` as data — finish with a value, park on spawned
  cells, await a binder, tail-call — and the drain does every create and
  release. The continuation is a koan enum: a native step (a function pointer
  over a state value, the shape a builtin body takes) and the component step.
  Evaluating an expression is [dispatch](dispatch.md)'s, so the scheduler takes
  one `evaluate` function at construction that turns a part and its activation
  into a continuation; the component step spawns an eager part through it, and
  the tests install a native stub.
- *Tree cells versus slab cells — decided.* Per
  [cellgraph/src/tree/README.md](../../cellgraph/src/tree/README.md): a call subtree is a
  stack discipline and takes tree cells; slab cells are for work whose
  liveness is not nested, which is the top-level statements. With slab cells
  that rare the matrix width drops to one word, 64 slots.
- *Statement cells of a called body — decided.* Each is a tenant of its frame,
  so binding a claimed slot is a plain write at `'here` and a reader woken on
  the binder re-reads the slot; the value never travels to the reader.
- *Where the top level's bindings live — decided.* In the region of a root
  slab cell, storage-only and never entered, that outlives every component.
  Only a cell can hold, and a hold is what lets a binder's region seal at
  retirement rather than reclaim; program storage
  ([src/scope/README.md](../../src/scope/README.md#three-tiers)) is no cell,
  and a `'graph` borrow cannot embed a `'here` one, so a top-level value kept
  there could only be a deep copy. The slot array itself does live in program
  storage: `'graph` is nameable from every step, so a claim, a wait record and a
  bind are plain `Cell` writes from any statement cell, and what a bound slot
  holds is a dormant carrier, which carries no brand. A binder builds its value
  through `alloc_into` the root, pinning what the value reaches, so the root
  holds the binder's region and that region seals at retirement with what the
  value reaches inside it, or the verdict copies and the region is reclaimed —
  a pricing decision, never a forced copy. A reader takes `hold` on the root
  once, which its pin row then names for its life, and redeems the slot's
  carrier at its own `'here`. The root's region and everything it holds live
  for the program, which is a top-level binding's real lifetime; what keeps
  that retention to the values is the scratch habitat, where a binder's
  intermediates go and which nothing ever seals. The destination forwards: a
  call whose result is bound for the root builds it there, and a *shares* call
  passes that destination to the argument it embeds, so in `x = cons(1, f(z))`
  at the top level `f` builds its result in the root's region for `cons` to
  embed. A *shares* call made at the top level has no frame to be a tenant of
  and is a tenant of its statement's slab cell. A `MODULE` or `GROUP`
  activation takes no cell of its own: it is data like any other value, placed
  by the ordinary crossing verdict in the region of whatever keeps it, so one
  bound at the top level goes to the root's region as any top-level binding
  does and one born inside a called function is built in its consumer and
  retained by the pin relation. Consequence for koan: `SlotArray` is
  instantiated at two habitats, `'graph` at the top level holding carriers and
  `'here` in a frame holding values, and they are distinct types, since a
  `Cell` is invariant in its brand.
- *What a body parks on — decided.* A frame and the slots it awaits, never a
  producer cell. A producer's handle does not survive a tail call, since the
  successor inherits the receipt, so a `Claimed` slot's producer
  ([src/memory/README.md](../../src/memory/README.md#the-slot-array)) can name a
  cell that is gone; a frame and a slot index name the same place for the whole
  call, and name it before the binder's cell exists. Every statement cell of a
  called body is a tenant of that frame, so the awaiter's wait record and the
  binder's read of it are plain writes at `'here` and the scheduler keeps
  nothing. A bind is a delivery — the binder fills each awaiter's receipt slot
  with a token and the awaiter re-reads the frame slot — so one rule wakes every
  parked cell, its receipt run is full, and a cell parked on both a pending
  binder and a sub-dispatch has one wake condition rather than a conjunction the
  drain would have to hold.
- *Errors — decided.* A lowered error is a tagged value
  ([src/values/README.md](../../src/values/README.md#what-a-value-is)), so no
  `Result` passes between cells and a consumer checks the results it redeems.
  The drain's own failures — cells parked with nothing runnable — are the only
  errors the scheduler defines.
- *Delivery — decided.* A producer pushes: it builds its result in its
  consumer — in the consumer's region when the result is bound for storage, in
  the consumer's scratch habitat when it is built fresh and only read, which is
  what a condition, a computed lookup key or a discarded statement value is —
  fills its receipt slot through `cellgraph`'s delivery doors
  ([cellgraph/src/receipt.rs](../../cellgraph/src/receipt.rs)), and dies. A result bound for storage is built in the consumer's region from
  the start, since a value that passes through scratch comes back at `'scratch`
  and can never be embedded in storage again; its receipt is the dormant
  carrier, which has no region brand and so rests in a scratch slot like any
  other. The scratch fill takes no operands, so a read-only result that borrows
  data already in the consumer goes as a carrier too. A scratch result is not
  short-lived by nature: it lasts across steps for as long as the consumer's
  scratch continuation names it, and never past the cell. The
  scheduler's own state is the work queue and the parked count: the outstanding
  count lives in the receipt run, each producer carries its consumer and slot
  index, and a tail hop hands that pair to its successor.
- *Slack in the slab — decided.* No reserve: admission fills the cap. Nothing a
  running component asks for takes a slab slot — its call subtree is tree cells
  and tenants under its own root, a stream at rest is a value
  ([yielding-iterators.md](yielding-iterators.md)), a module activation is
  data, and a top-level binding lives in the root — so beside the root's own
  slot the cap bounds how many components run at once and nothing else.
- *Why admission cannot deadlock — decided.* An eager mention reads at its
  statement's position and never sees a later binding, and a forward use is an
  unbound-name error
  ([src/scope/README.md](../../src/scope/README.md#visibility)); a deferred
  mention that does reach forward becomes a knot edge rather than a wait, and
  its component is one cell. So a component waits only on lower positions, and
  position order is a topological order. The admitted set is contiguous from
  the oldest unreleased component, so everything below the lowest admitted one
  has completed and that component is runnable: the drain always has work, at
  any cap. The cap is a throughput knob, not a correctness one.
- *Threads — decided: not designed here, not precluded.* The unit would be the
  top-level component, one graph per thread: `cellgraph` is not `Send`, and a
  step holds the region table shared while every death verb takes it
  exclusively, so threads in one graph would serialize on every release. What
  that needs and this item does not build is a top-level slot array that is
  `Sync`, a binding readable from a graph other than the one whose root holds
  it, and a wake that crosses graphs, since a bind is a delivery and a delivery
  reaches one graph's cells. What this item owes it is per-graph state and a
  call subtree that stays in the graph that admitted it.
- *Own region versus the caller's region for a sub-dispatch — decided.*
  `cellgraph` ships a tenant cell — one with no region of its own, whose writer
  is its host's — and the scheduler elects it per spawned cell from a one-bit,
  per-function hint: whether the function's result shares structure with its
  arguments. A fresh result is built in a tree child with its own region and
  placed operand-free into its destination, so the child reclaims whole at
  death; a sharing result is built by a tenant of the caller's frame, which
  embeds its arguments at no price. The same bit decides a tail hop: a fresh
  hop draws a recycled region and the loop runs in bounded memory, a sharing
  hop joins its predecessor's region, whose growth the result retains anyway.
  Bounded memory has a price the bit also carries: sibling tree cells are
  `Apart`, so a fresh hop's arguments homed in its predecessor cross by a forced
  copy, every hop, while an argument homed in the caller's frame or above is
  `Under` and free. A loop that threads a large state it built itself wants
  *shares* whatever its return type says.
  The hint is never a contract — a wrong *fresh* costs a priced copy, a wrong
  *shares* costs delayed reclaim, and soundness rests on the substrate's brands
  either way. First cut: builtins declare the bit and a user function derives
  it from its return type (a flat return cannot share). Open beyond the first
  cut, in precedence order over the derived bit: a programmer annotation with
  no semantic effect, since the cost of a wrong guess scales with data size
  only a programmer can predict; and a runtime measurement of copied and
  retained bytes per function.
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
- *How a tail hop's arguments reach the successor — open.* They are homed in the
  predecessor, which must outlive the hand-off, so the successor exists before
  the release. Recommended: the predecessor's step puts them to rest with `keep`, the drain creates
  the successor with the dormant carriers in its continuation, and the
  successor's first step redeems them — entitled by root identity, which covers
  a sibling — and copies them in through `alloc_here` before the drain releases
  the predecessor. The alternative, the predecessor placing them with
  `alloc_into`, costs it a second step after the successor's birth.

## Dependencies

This item subsumes `workgraph`'s own
[adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md), which stays
as a requirements record for the module. [Values](../../src/values/README.md),
what a cell delivers, and cellgraph's [tenant cells, scratch habitat, receipt
runs and recycled regions](../../cellgraph/README.md#the-cell) already ship, as
do the [delivery doors](../../cellgraph/README.md#passing-values-between-cells)
a producer fills its consumer's receipt through.

**Requires:** none — the substrate it stands on ships.

**Unblocks:**

- [Dispatch](dispatch.md) — running a program needs the scheduler that drives it.
- [Yielding iterators](yielding-iterators.md) — a flat consumer loop is its tail call.

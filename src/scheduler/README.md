# The scheduler

The drain koan runs on: a veneer over
[`cellgraph`](../../cellgraph/README.md)'s cells, reached only through
[`memory`](../memory/README.md). It sits above [`values`](../values/README.md)
and [`knot`](../knot/README.md), and neither of them names it.

A unit of work **is** a cell. Its region, its erased continuation, its receipt
run and its holds are the cell's, and the scheduler keeps no second record of
any of them. What it owns is a **ready stack** and the protocol a step ends by.
What it offers the layers above is a way to say what a computation needs — a
child, a wait, a successor, a result — beside the hints that place it, without
saying which cell, which door or which region that becomes.

Four things it does not keep, because a neighbour already does:

- **No order of its own.** The order work runs in is the order it was asked
  for. The dependency order of a body's bindings is the
  [shape](../scope/README.md#visibility)'s, computed once where the shape is
  built.
- **No wait record.** The only thing a live cell waits on is the receipt run
  the substrate lays down for it, and the outstanding count is that run's.
- **No liveness.** No reference count, no tally of live cells, no pin bundle
  and no antichain fold: a cell is reclaimed the instant no hold names it, and
  `cellgraph`'s `is_empty` is the one alarm.
- **No value at rest.** No envelope and no mailbox holds a value outside a cell.

## The drain

The graph a drain runs over is `Graph`: a newtype over a `CellGraph` closed over
the continuation family, the scratch-state family and the delivery bundle, so
the families stay the scheduler's own. What owns a graph makes one with
`Graph::new`, takes a storage-only slab cell from `Graph::root` — a region that
outlives every drain over it and is never entered — gives it back with
`Graph::release_root`, and asks `is_empty` and `is_live`. Every other cell is
the drain's.

`Scheduler` owns no graph. It is a view made per call: `Scheduler::over` borrows
a `Graph` mutably and keeps the ready stack and a buffer of requests beside it.
Whatever owns the graph keeps it across calls — a loaded
[program](../program/README.md) or a test's plain local — and a later view over
the same graph finds every cell and every root an earlier one left. Every field
is that view's own: no static, no thread-local and no lazily minted cell, so a
second scheduler over a second graph runs beside the first with nothing shared
but the program text and the shapes in it.

`Scheduler::run` takes one **root work**, the cell it is born under and the
placement it is born at, and drives it to its end. Everything else a drain runs
is a descendant the root work asked for. The root work reports to nobody, so it
ends well in one of two ways: `done`, or `leave`, which rests a birth in its
home for a later root work. `run` hands what was left back as an opaque,
brand-free `Resting`, and `Scheduler::resume` births a later root work from it
under the same cell — a tenant of that cell wakes it free. This is how the
graph's owner reaches what one drain built from a later one without a cell that
outlives the drain: a loaded [program](../program/README.md) leaves its top
level's view at rest and inspects a binding through a resumed root work.

### The ready stack

An entry of the stack is a live cell with a step to run, or a **request not yet
born**: the parent it will be born under, the slot it reports to and the work
it runs. The loop pops an entry, gives an unborn one its cell, enters the cell,
runs the step its continuation names, and acts on the `Kind` the returned
`Action` opens into:

- `Finished` — any delivery the step made has already filled its consumer's
  receipt, so nothing is in flight when the drain releases the cell. When that
  fill completed the consumer's receipt run, the drain pushes the consumer, read
  off its own copy of the cell's provenance. This is the only way a parked cell
  wakes.
- `Park` — the step registered a receipt run and asked for children. The drain
  pushes their requests so the first asked is on top, and leaves the cell alone
  until its receipt run completes.
- `Tail` — the drain pushes the successor and releases this cell once that
  successor's first step has run.
- `Failed` — the drain abandons the work.

A stack makes the drain **depth-first**. The child asked first runs to
completion — its own children, their children, and its delivery — before the
child asked second is born. Three things follow.

- **Evaluation order is the order of asking.** Two arguments evaluated by one
  park run one after the other, never step by step in turn, so their effects
  do not interleave. A layer that asks in source order gets source order.
- **The cells live at any moment are one path**: the running cell, its parked
  ancestors, and one predecessor during a tail hop. A sibling whose turn has not
  come has no cell and no region, so a recursion that asks for two children at
  every level holds cells in proportion to its depth, and a body of a thousand
  statements never holds a thousand cells.
- **A cell that waits on an earlier sibling needs no wait.** It is born after
  that sibling finished.

The stack emptying with the root work ended is success. The stack emptying
first is `DrainStalled::Unfinished`: some cell is parked on a receipt run
nothing will fill. A birth, an enter or a release the substrate refuses is a
`DrainStalled` of its own, and a step that cannot proceed is
`DrainStalled::Step` — among them a step that asked for children and ended
without parking on them, `StepError::Unparked`, and a step the layer above
refused, `StepError::Refused`, whose reason is that layer's. These are the only
errors the scheduler defines. A koan
error is a [tagged value](../values/README.md#what-a-value-is) and travels
between cells as data, so no `Result` passes from one cell to another and a
consumer checks the results it reads.

A view dropped before its root work ended abandons what is on its stack. The
graph keeps the cells already born, under the root they were born under, and
that root's release does not empty the graph — which is what `is_empty` reports.
A later `run` over the same view clears the stack before it births its root
work, so nothing a stalled run left there is ever popped.

## What a step may name

`enter` quantifies a step's three brands — `'step`, `'here` and `'scratch` — per
call, so a step's return type can name none of them. That is why `Action`
carries no region borrow, and why the children a step asks for go into a
drain-owned buffer rather than into what it returns.

A step is handed a `Step` by value and nothing else. `Step` borrows the raw
`StepContext`, the cell's `Provenance` and the drain's request buffer, all
private, holds the state its cell was woken with and the scratch state the
previous step parked, and exposes:

- its cell's two writers, `writer` at `'here` and `scratch_writer` at
  `'scratch`;
- `state` and `scratch`, which take the cell's state and its parked scratch
  state, each once. A state is a projection of the step bundle, which a
  higher-ranked function pointer cannot take as a parameter (rustc #100013), so
  a step takes both from the `Step` rather than as arguments beside it. The
  once is a type: `Step` carries one marker per state, `Holding` or `Taken`,
  both defaulting to `Holding` so a step's own signature names neither. Each
  take hands the value back beside the `Step` in its `Taken` form, which has no
  second take to call. A step that never takes its scratch state leaves it in
  the `Step`, and the end hands its bump back unless the step parks again;
- `results`, the children's results it parked on;
- `spawn`, to ask for a child;
- the ends — `park`, `tail`, `finish_fresh`, `finish_in_home`, `finish`,
  `done`, `leave` and `failed`.

**No step names a place that is not its own.** There is no handle among those
doors — not the cell's own, not its consumer's, not its result's home — and no
carrier door: no `alloc_into`, `lift`, `keep`, `redeem` or `receipt`. A step
holds values at its own brands and nothing else. Each crossing a computation
needs is the veneer's to perform, from the provenance the drain filled:

- **A child's birth is handed over awake and arrives awake.** `spawn` and
  `tail` take the birth the new cell starts from as the step holds it, at
  `'here`. The veneer puts it to rest as a dormant carrier in the birth
  continuation, and on the new cell's first entry redeems it and crosses it to
  that cell's `'here` at the [verdict](../values/README.md)'s price — free for
  a child, whose spawner is above it, and free for a tenant, which crosses
  nothing.
- **What a root work leaves crosses into its home.** `leave` crosses the birth
  into the cell the root work was born under at the verdict's price, not into
  its own region, which a `Fresh` root work's release reclaims; from a tenant of
  that cell it crosses nothing. It is refused for a cell that reports to
  somebody.
- **A child's result is read, not redeemed.** `results` yields each slot of the
  receipt run as the consumer can use it: a scratch fill at `'scratch`, a
  carrier fill redeemed and crossed to `'here`.
- **A result goes where the drain said.** The three `finish` ends below build
  or cross into the home the provenance names and fill the slot it names.

Each end consumes the `Step`, and `Action` is opaque, a private field over an
internal `Kind`, with no other constructor. So a step ends exactly once, nothing
it does follows its end, and the bookkeeping an arm implies has always happened
by the time the drain reads it. `Step::park` takes the `Slot` the last `spawn`
handed back, so a park with nothing to wait on does not typecheck; it registers
a receipt run sized by the spawns so far, stores the successor under the cell's
own provenance, and stores the scratch state when it is given one. The drain
took the scratch slot off before the step began and nothing else fills it, so a
park that carries nothing there leaves it empty and that step's end hands the
bump back.

A step also cannot create or release a cell. Neither `Step` nor the
`StepContext` behind it has a `create`, a `release` or a second `enter`, so
every birth and every death is the drain's, performed from the `Action` the
step handed back.

## Hints

A `Request` is a `Work` — the step the child starts at and the birth it starts
from — and two hints. Neither is a contract: a wrong hint costs a copy or a
delayed reclaim, and soundness rests on the substrate's brands either way.

**`Placement`** picks the child's habitat.

| `Placement` | door | the child's region | the spawner's price |
|---|---|---|---|
| `Fresh` | `create_tree` | its own, reclaimed whole at death | a result placed operand-free crosses free |
| `Shares` | `create_tenant` | none — the spawner's, at the spawner's own brand | an embedded argument crosses free, because nothing crosses |

The same bit decides a tail hop. A `Fresh` hop draws a recycled region and the
loop runs in memory independent of hop count; a `Shares` hop joins its
predecessor's region, whose growth the result retains anyway. Bounded memory has
a price the bit also carries: sibling tree cells are `Apart`, so a `Fresh` hop's
state homed in its predecessor crosses by a forced copy, every hop, while state
homed in the caller's frame or above is `Under` and free. A loop that threads a
large state it built itself wants `Shares` whatever its return type says. A
tenant's scratch is its host's, so a tenant writes its intermediates there while
a result bound for storage is built in host storage from the start.

**`Use`** says what the spawner will do with the result, which only the spawner
knows, and so where the result's **home** is.

| `Use` | the spawner will | the result's home |
|---|---|---|
| `Reads` | inspect it and drop it — a condition, a lookup key, a discarded statement value | the spawner's scratch where the result is fresh, else the spawner |
| `Keeps` | embed it, bind it or hold it across parks | the spawner |
| `Forwards` | embed it in its own result | the spawner's own home |

`Forwards` is destination passing: in `cons(1, f(z))` the call of `f` is asked
for with `Forwards`, so `f` builds in the region `cons`'s result is bound for
and `cons` embeds it at no price. The home travels down a chain of forwards and
across a tail hop unchanged.

## Delivery

A producer pushes. It builds its result in its home, fills its consumer's
receipt slot through `cellgraph`'s
[delivery doors](../../cellgraph/README.md#passing-values-between-cells), and
dies. The producer says what shape its result has by which end it takes, the
spawner said what it is for by the `Use` it asked with, and the veneer picks the
door from the two.

| end | the producer has | under `Reads` | under `Keeps` or `Forwards` |
|---|---|---|---|
| `finish_fresh(build)` | a build that takes no operands | built in the consumer's scratch habitat, `deliver_scratch` | built operand-free in the home, filed as a carrier |
| `finish_in_home(operands, build)` | a build over values it holds | built in the home, each operand [pinned or severed](../../cellgraph/README.md#the-crossing-verdict) as the verdict ruled, filed as a carrier | the same |
| `finish(value)` | a value already at `'here` | crossed into the home at the verdict's price, filed as a carrier | the same |

A result that passes through scratch comes back at `'scratch` and can never be
embedded in storage again, which is why only `Reads` reaches that door, and
because the scratch build takes no operands only `finish_fresh` does. A scratch
result is not short-lived by nature: it lasts for as long as the consumer's
scratch state names it, and never past the cell. A tenant's result is already in
its home when that home is its host, so its `finish` crosses nothing.

The outstanding count lives in the receipt run, each producer carries its slot
in its `Provenance`, and a tail hop hands the provenance to its successor.

## The continuation

A cell parks in two slots, one per habitat, each its own family, so the wrong
form is unrepresentable in each. The continuation family re-anchors at the
executing cell's region brand and is what the drain reads to run a step; the
scratch family is over **both** step brands and carries a parked cell's
in-progress state in the scratch habitat, so what it names in storage comes back
at `'here` and what it names in the habitat at `'scratch`.

The scheduler names no step and no state of the layers above it. It takes them
as one **step bundle**, a parameter beside the delivery bundle, with three
families:

- the **birth** family — what a spawn or a tail hands a new cell, and what a
  root work is born with or leaves at rest. A birth crosses pinned from a
  spawner into its child, so the family is
  [`Covariant`](../../cellgraph/src/reattach.rs), and the bundle supplies its
  crossing: a weight, and a copy over the crossed view a placement hands it;
- the **parked state** family — what a cell holds at a step and parks with. A
  park stores it in the cell's own continuation and the wake hands it back at
  the same cell's brand, so it never crosses, need not be covariant, and may
  hold a borrow a crossing would refuse, such as an invariant activation;
- the **scratch** family over both brands, which rests in its own cell and
  never crosses.

`born` joins the first two: the state a cell's first step runs over, made from
the birth it was handed. The veneer rests a birth by `lift` then `keep`, which
needs nothing of the family, and wakes it by a redeem and an own-cell crossing
priced by the verdict, which needs both halves of the crossing and calls them
only inside that priced crossing. A continuation is one shape with no arm to
add — a `NativeStep` function pointer, the cell's `Provenance`, and the
bundle's parked state, or a birth at rest. The pointer is higher-ranked over the
three step brands and not over `'graph`, so one pointer runs in any cell at any
step while the state it reads, the children it asks for and the action it
returns all name the graph they belong to.

A *birth* continuation is the family at `'cell = 'graph`, because every birth
door takes one there: it holds only words, program storage and dormant
carriers, which is the rested form the veneer makes of what `spawn` and `tail`
were handed. The successor `Step::park` stores is at `'here` and may hold region
borrows freely.

`Provenance` is what a cell carries about itself, for the drain's use:

- the **parent** it was born under, which is also the consumer it reports to
  and the place a successor of it is born beside;
- the **slot** of that parent's receipt run it fills and the `Use` it was asked
  with, absent for a root work;
- the **home** its result is built in.

Every field is brand-free, so it survives a tail hop verbatim — which is what
"the successor inherits its receipt" means. The drain fills it from how a
request was handed over: given to `Step::spawn`, the child is born under the
spawner and reports to the slot of its spawn position; given to `Step::tail`,
the successor inherits its predecessor's provenance. A step never sees it.

## How a cell waits

One way: parked on its receipt run, for children it asked for. The count lives
in the substrate's own run, a producer's delivery decrements it, and the
consumer wakes once, when the last slot fills.

The dependencies between a body's bindings are not a wait. A
[shape](../scope/README.md#visibility) knows its body's reference graph before
the body runs and numbers the body's units so that each follows every unit it
reads, and a drain that runs what it is asked in the order it is asked turns
that numbering into the run-time order. A reader therefore never observes a
binding whose binder has not run, with no count, no edge, no waiter list and no
park on a slot. An outside event reaches a koan program only through a form the
shape can see, so nothing at run time can make a unit ready in any order but
the one the shape gave.

## Tail hops

A tail call is a create plus a release, in that order. The successor is born a
sibling of its predecessor or a co-tenant of its host, and inherits its
provenance; the predecessor is released once the successor holds everything
that was homed in it. The hand-off:

1. The predecessor ends with `Step::tail`, a placement, the successor's work
   and its birth, awake.
2. The veneer rests that birth into the successor's birth continuation, and the
   drain pushes the successor and holds the predecessor aside.
3. The drain creates the successor — `create_tree` under the predecessor's own
   parent for `Fresh`, `create_tenant` on the same host for `Shares` — and
   enters it.
4. The veneer wakes the birth, entitled by root identity, crossing it to the
   successor's `'here`, and the successor's first step runs over it.
5. Only then does the drain release the predecessor.

Step 5 is why the release is not part of step 3: a released cell with no pledge
reclaims its bump, and the bytes the successor is about to redeem would be
gone. The successor is on top of the stack, so its round is the very next one
and at most two hops of a loop are live at once.

Every cell a drain runs is born under a parent, so every cell can hop, and a
hop is a tree cell or a tenant, never a slab cell. A slab successor would have
to `hold` its predecessor to redeem, which pins the predecessor's region into
its row, so the release would seal rather than reclaim and constant occupancy
would be lost. Tree siblings share a root, and root identity is the
entitlement — no hold, no pin, no seal.

## Memory

The slab holds roots and nothing else: the tree pool takes no cap, so a call
subtree deeper than any slab cap runs on tree cells without touching the matrix.
The matrix width is therefore one word — `WIDTH` in
[substrate.rs](../memory/substrate.rs) is `1`, sixty-four slab slots.

The view's heap is the ready stack and the request buffer. Both are amortized,
neither grows per step once warm, and the stack's depth is the unborn siblings
along the current path. Everything a computation keeps across a park is in its
cell: the continuation in storage, the in-progress state and the receipt run in
the scratch habitat, handed back whole at the first step end that leaves
nothing naming it.

## The import rule

Outside doc comments and `#[cfg(test)]` this module names `crate::knot`,
`crate::memory` and `crate::values`, and nothing else in the crate. It does not
name `scope`, `parse`, `elaborate` or `program`. `cellgraph` is reached only
through `memory`, and `cellgraph` itself depends on neither this module nor
koan. `tests/boundary.rs` reads the source to hold the rule there; the ready
stack and the request buffer are the owning heap types it blanks, because each
is the scheduler's own runtime state and never a value in a region.

## Testing

The tests drive the drain with native-step workloads over a test bundle, with
no layer of koan's above present. They hold, each with a workload of its own:

- a call at each placement, and a result at each `Use` through each end, read
  back at the brand the table above gives it;
- depth-first order — two children asked in one park, each with children of its
  own, record their steps one subtree after the other — and the live path: a
  recursion asking two children per level peaks at cells in proportion to its
  depth, and a hundred siblings asked in one park run one cell at a time;
- a consumer parked on several producers waking once, and a cell gathering its
  children's results into a run in its scratch habitat across two parks;
- a ten-thousand-hop loop at each placement, with three cells live at the peak
  and no more heap than the same loop a hundred hops long;
- a subtree two hundred levels deep on a slab of one;
- a root work that leaves a birth a later root work resumes from, at each
  placement, and a cell that reports to somebody refused on `leave`;
- two schedulers beside each other sharing nothing, two views over one graph in
  turn, and a view dropped mid-work leaving a graph its root's release does not
  empty;
- the round trip a continuation makes between `'graph` and a step's `'here`,
  through the raw slot doors a step never reaches.

A native step is a bare `fn` and carries no closure state, so what a step
observes it records in `tests/native.rs` for the test around it to read back.

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

The graph a drain runs over is `Graph`: a `CellGraph` closed over the
continuation family, the scratch-state family and the delivery bundle, so the
families stay the scheduler's own, beside the type tiers every step's chain
reads first (`TypeTiers`: the frozen load and the agent tier), the load's
facts every step's door reads beneath its arena node, and the
[build arena](#the-build-arena). What owns a graph makes one with
`Graph::new` over those tiers — a loaded program over the
load it froze and the agent tier laid beside it, a test over ones it seeds in
a local bump — takes a storage-only slab cell from `Graph::root` — a region that
outlives every drain over it and is never entered — gives it back with
`Graph::release_root`, and asks `is_empty` and `is_live`. Every other cell is
the drain's.

A loaded program's graph is stood up instead by the **load door**,
`Graph::load` ([load.rs](load.rs)): it runs the caller's load closure first,
outside any cell, and only then stands the graph over one slab root, the
**run root**, where every root work is born. The closure is handed nothing,
and returns the type tiers, the load's facts and the door a miss settles
through ([`Settles`](seek.rs)); a refusal it returns stands no graph. The
load door also takes the arena's budget. A graph `Graph::new` makes runs no
program: its facts are empty and every settlement it builds holds none.

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
- `Tail` — the drain pushes the successor, born under this cell for a
  hand-off and beside it for a hop or a rebirth ([tail hops](#tail-hops)),
  and releases this cell once that successor's first step has run.
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

`enter` quantifies a step's four brands — `'step`, `'here`, `'scratch` and
`'lent`, a tree cell's borrow of its parent's scratch — per call, so a step's
return type can name none of them. That is why `Action`
carries no region borrow, and why the children a step asks for go into a
drain-owned buffer rather than into what it returns.

A step is handed a `Step` by value and nothing else. `Step` borrows the raw
`StepContext`, the cell's `Provenance` and the drain's request buffer, all
private, holds the state its cell was woken with and the scratch state the
previous step parked, and exposes:

- its cell's two writers, `writer` at `'here` and `scratch_writer` at
  `'scratch`, the `header` door of its write home, and the type chain beside
  each writer — `types` and `scratch_types`, built over the graph's type tiers
  once per step and never parked — so a type the step builds lands in the
  branch of the habitat it lays its value in;
- the chain a [caching body](../program/README.md#caching-bodies) runs on,
  paired to the region writer: `caching_types`, over the write home's region
  and scratch, its own tier in scratch, which a tree cell holds as its own and
  a tenant shares with its host. A frame, always a tree cell, routes once
  `hold_scratch` has held its scratch for the cell's life. No chain reads a
  lent half, and none writes the
  agent tier's types. `results_with` and
  `finish_with` run a pinned value's debug check against such a chain;
- its **door**, the code and the facts of the base its chain rests on beneath
  the settlement it runs under, borrowed from the arena for the step alone,
  so what a cell parks names a segment, a site or a digest instead; and the
  decision memo's doors, `memo_entry` and `memo_lay`, over the home its
  result is built in ([the build module](../build/README.md#activations));
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
- `spawn`, to ask for a child; `spawn_seeking`, to ask for one born under the
  arena node a `Seek` finds or builds; and `spawn_composing`, to ask for one
  born under the composition of code composed at run time
  ([below](#the-build-arena));
- `arena_ref`, the arena node the cell runs under;
- `sought`, at a call's first step, the request the drain hands back for the
  frame to lay its level from ([below](#the-build-arena)), taken once;
- `kept_here`, which keeps a value the cell holds at the cell itself
  ([below](#kept-and-held-values));
- the ends — `park`, `hand_off`, `tail`, `tail_seeking`, `tail_under`,
  `bounce`, `rebuild`, `replay`, `finish_fresh`, `finish_in_home`, `finish`,
  `done`, `leave` and `failed`.

**No step names a place that is not its own.** There is no handle among those
doors — not the cell's own, not its consumer's, not its result's home — and no
carrier door: no `alloc_into`, `lift`, `keep`, `redeem` or `receipt`. A step
holds values at its own brands and nothing else. An arena node travels above
the scheduler as an `ArenaRef`, an opaque handle only the scheduler and
`values` open: a step is handed one by `arena_ref` and hands one back in a
`Seek` or to `tail_under`, so it can name to the drain only an arena node the
scheduler named to it, and a handle gone stale names nothing. Each crossing a computation needs is the veneer's to
perform, from the provenance the drain filled:

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
  carrier fill redeemed and crossed to `'here` — held beside its carrier,
  kept, where it was pinned.
- **A crossing carries types.** `results`, `finish`, `finish_in_home` and the
  wake of a rested birth build the destination's chain inside the build, over
  the graph's type tiers and the build's writer and header, since a
  [crossing](../values/README.md#crossing) makes every type the value carries
  resolve there and a deep copy lays each part down at the type it is seen at.
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

### Kept and held values

A carrier's home is the cell that lifted its value, not where the value's
bytes are: a part of a parameter, lifted in a frame, is homed in the frame.
So a step that crossed a value on by lifting it would pin or copy from its
own cell, never from where the value rests. A **kept** value
([kept.rs](kept.rs)) is the fix: an opaque, brand-free `Kept`, a carrier put
to rest at the value's true home, which only the veneer redeems. A step makes
one itself only with `kept_here`, which keeps a value the cell holds — one it
was handed by a copy, or built — at the cell itself. A `KeptPart` is a kept
value and a path of segments down to one of its parts, projected through
cellgraph's `project`. Each segment is one step down: a record field's label,
read as `Value::along` reads it; a function's capture slot, read as
`Value::captured` reads it, an edge resolved to the fellow member of the knot
it names; or a module's member slot, which `values` sees as opaque and the
knot reads (`layout::stored_member`).

**The wake keeps what every call hands on.** The bundle lays every call's
birth beside the kept part of each value it hands on, the callee among them
(`StepBundle::handings`), and the wake keeps each at its true home, so a
frame's first step finds what it keeps of its callee and arguments beside its
birth and projects nothing out of it. A hop's successor crosses each value on
its own ([tail hops](#tail-hops)). A spawned child's spawner is above it, and
so is every home a value it is handed is kept at, so the child pins its birth
whole: the wake keeps each value from the kept part it was handed through
where that redeems, and otherwise projects it out of the birth and keeps it at
the birth's home, and hands what it kept to the bundle beside the pinned birth
(`StepBundle::keeping`). A birth crossed by a copy is other storage than what
was kept, so its frame keeps afresh.

Every value an evaluation holds is a `Held`: the value, and the kept part it
is, or is a restamp of, where it has one. A pinned child result arrives held
beside its own carrier. The invariant — the value is the storage the kept
part reaches, possibly seen at another type — is checked in a debug build
wherever the veneer redeems one. The ends read it: `finish` hands a held
value whose kept home lies at or above the destination on unchanged
(cellgraph's `pass_into`), and `finish_in_home` crosses each operand from its
kept part where it has one ([delivery](#delivery)). What a frame keeps, and
how a name read finds it, is the
[program's](../program/README.md#frames-contracts-and-tails).

## Hints

A `Request` is a `Work` — the step the child starts at and the birth it starts
from — and two hints. Neither is a contract: a wrong hint costs a copy or a
delayed reclaim, and soundness rests on the substrate's brands either way.

**`Placement`** picks the child's habitat.

| `Placement` | door | the child's region | the spawner's price |
|---|---|---|---|
| `Fresh` | `create_tree_lent` | its own, reclaimed whole at death | a result placed operand-free crosses free |
| `Shares` | `create_tenant` | none — the spawner's, at the spawner's own brand | an embedded argument crosses free, because nothing crosses |

A tail takes no placement. A hand-off and a hop are always `Fresh` tree cells
— a hand-off a child of its frame, a hop a sibling of its predecessor under its
loop cell or its parent — and only a rebirth (`bounce`, `rebuild`, `replay`)
keeps the placement its cell was born at ([tail hops](#tail-hops)). Sibling
tree cells are `Apart`, so a hop's state homed in its predecessor crosses by a
forced copy, while state homed in the loop cell, the caller's frame or above is
`Under` and free. A tenant's scratch is its host's, so a tenant writes its
intermediates there while a result bound for storage is built in host storage
from the start.

**`Use`** says what the spawner will do with the result, which only the spawner
knows, and so where the result's **home** is.

| `Use` | the spawner will | the result's home |
|---|---|---|
| `Reads` | inspect it and drop it — a condition, a lookup key, a discarded statement value | the spawner's scratch where the result is fresh, else the spawner |
| `Keeps` | embed it, bind it or hold it across parks | the spawner |
| `Forwards` | embed it in its own result | the spawner's own home |
| `Threads` | hand it on in a tail call, so it outlives the hop | the spawner's loop cell, else the spawner |

`Forwards` is destination passing: in `cons(1, f(z))` the call of `f` is asked
for with `Forwards`, so `f` builds in the region `cons`'s result is bound for
and `cons` embeds it at no price. The home travels down a chain of forwards and
across a tail hop unchanged. `Threads` is what a frame asks a value of its
body's [threaded set](../scope/README.md#the-threaded-set) with; a home it names in
a loop cell copies whatever a hop homes, at any price, so no hop is ever
pledged into the loop cell.

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
| `finish_in_home(operands, build)` | a build over values it holds | built in the home, each operand crossed from its kept part where it has one and [pinned or severed](../../cellgraph/README.md#the-crossing-verdict) as the verdict ruled, restamped at the type it is held at, filed as a carrier | the same |
| `finish(value)` | a value already at `'here` | handed on unchanged where its kept part's home is the home or above it; else crossed into the home at the verdict's price, filed as a carrier | the same |

A result that passes through scratch comes back at `'scratch` and can never be
embedded in storage again, which is why only `Reads` reaches that door, and
because the scratch build takes no operands only `finish_fresh` does. A scratch
result is not short-lived by nature: it lasts for as long as the consumer's
scratch state names it, and never past the cell. A tenant's result is already in
its home when that home is its host, so its `finish` crosses nothing. A value
handed on unchanged keeps its home, so a call that returns its parameter, or a
field of one, places nothing new in its caller.

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
were handed. A birth at rest keeps how it was handed over, a `Born`: a spawned
child, a root work resumed from what an earlier one left, a hop's successor,
or what a root work leaves. The wake hands it to the bundle's crossing, which
for a pinned birth links the cell's branch to its home's where the cell has
laid no branch — a spawned child always, a hop's successor or a left birth when
it lands in a new cell — and merges the home's branch into one already laid, a
tenant's host's ([crossing](../values/README.md#crossing)). The successor `Step::park` stores is at `'here` and may hold region
borrows freely.

`Provenance` is what a cell carries about itself, for the drain's use:

- the **parent**, the consumer it reports to: the cell it, or the first cell
  of its tail chain, was born under. With no loop cell, a hop is born under it
  too;
- the **slot** of that parent's receipt run it fills and the `Use` it was asked
  with, absent for a root work;
- the **home** its result is built in, and whether that home is a loop cell
  (`threads`), so a value built there copies whatever a hop homes;
- the **loop cell** a tail loop it runs in threads its state through, where
  its hops are born, beside the bytes the loop cell held one hop after it was
  filled (`loop_base`), which compaction weighs it against
  ([tail hops](#tail-hops)). `None` for a spawned cell.

So three cells play three roles: `parent` receives the result, `loop_cell` —
else `parent` — births a hop or a rebirth, and a frame's hand-off is born under
the frame itself. Every field is brand-free, so it survives a tail hop verbatim
— which is what "the successor inherits its receipt" means. The drain fills it
from how a request was handed over: given to `Step::spawn`, the child is born
under the spawner and reports to the slot of its spawn position; given to a
tail end, the successor inherits its predecessor's provenance. A step never
sees it.

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

A tail call is a create plus a release, in that order. The successor inherits
its predecessor's provenance, and the predecessor is released once the
successor holds everything that was homed in it. Three ends tail, each
deciding where the successor is born:

- **A hand-off** (`hand_off`): a frame hands its last statement to an
  evaluation born under the frame, as its `Fresh` tree child. The evaluation
  views the frame's activation from below it, so the view pins free and
  pledges nothing.
- **A hop** (`tail`, `tail_seeking`, `tail_under`): a call's frame born
  `Fresh` beside its predecessor, under the provenance's loop cell, else its
  parent.
- **A rebirth** (`bounce`, `rebuild`, `replay`): the same work born again
  beside the cell, at the placement the cell was born at.

The steps:

1. The predecessor ends with one of those, the successor's work and its birth,
   awake.
2. The veneer rests that birth into the successor's birth continuation, and the
   drain pushes the successor and holds the predecessor aside.
3. The drain creates the successor where its end says and enters it.
4. The veneer wakes the birth, entitled by root identity, crossing it to the
   successor's `'here`, and the successor's first step runs over it.
5. Only then does the drain release the predecessor.

Step 5 is why the release is not part of step 3: a released cell with no pledge
reclaims its bump, and the bytes the successor is about to redeem would be
gone. The successor is on top of the stack, so its round is the very next one
and at most two hops of a loop are live at once — beside a frame released
before the evaluation it handed off to, which stays dead but undisposed until
that child disposes.

Every cell a drain runs is born under a parent, so every cell can tail, and a
successor is a tree cell or, for a rebirth, a tenant, never a slab cell. A slab
successor would have to `hold` its predecessor to redeem, which pins the
predecessor's region into its row, so the release would seal rather than
reclaim and constant occupancy would be lost. Tree siblings share a root, and
root identity is the entitlement — no hold, no pin, no seal.

**A hop hands on a skeleton.** A hop's predecessor and successor are siblings,
so a birth crossing whole between them is a forced copy of everything it
reaches, parts homed far above included. Where the bundle laid the birth as a
skeleton beside the values it hands on (`StepBundle::handings`), the wake
crosses each value on its own instead: from the kept part it was handed
through where that redeems, pinned free where its home is an ancestor of the
successor, and otherwise out of the skeleton. A value that does not pin is
copied into the successor — save, under a loop cell, a threaded one or one
handed through a kept part, which is copied into the loop cell and pinned from
there, so a value an earlier hop left in a loop cell rides each compaction into
the fresh one rather than into every hop after it. A callee handed on bare
threads, so the loop's frames keep it, and every capture read through it, in
the loop cell. Threaded values cross
first, so the successor's type branch links the loop cell's and every later
merge stops at what that link reaches, rather than entering a node per value
the loop has built. The successor keeps a carrier of each — at its home where
it pinned, at the successor where it was copied — and the bundle builds the
birth in the successor's region over them (`StepBundle::hop`). Nothing of the
skeleton outlives its predecessor.

**The loop cell.** A loop whose hops hand on values their frames built — the
values each frame asked for with `Threads` — threads them through a **loop
cell**: a tree cell under the loop's parent, with no continuation and no arena
borrow, that is the parent of every hop after it is made and the home those
values are built in. What it holds pins into each hop free, by ancestry. The
drain makes it at the first hop that threads (`Tailing::Hop` carrying the
weight its birth threads, `StepBundle::threads`) when the provenance names
none; the first frame's threaded values are copied into it once, there. A loop
that threads nothing and hands every callee through a kept part — a walker, a
scalar loop — gets none, and its hops stay siblings under its parent. Every hop and hand-off inherits the loop cell; a
spawned cell never does.

**Compaction.** A loop cell gains what each hop threads and keeps what earlier
hops threaded, so a loop that rebuilds its state whole would grow it by a
generation per hop. The drain weighs it at each hop against its live bytes:
the bytes it held one hop after it was filled, or the weight the hop threads,
whichever is more. Once it holds more than twice that, plus a region's first
chunk, the hop's successor is born under a fresh loop cell, the values it is
handed from the old one are copied into it, and the old one is released after
the successor's first step. Each compaction copies the live state once and the state has doubled
since the last, so a loop copies a constant per hop, amortized.

**Release at delivery.** The loop's last cell finishes like any other: its
result crosses into its home at the verdict's price, pledging the loop cell
upward or copying out of it. The drain then releases the loop cell.

## The build arena

The [build's](../build/README.md#where-facts-are-decided) books live in a
**build arena** the graph owns beside the cell graph ([arena.rs](arena.rs)),
so the cell graph holds run cells alone and the drain reads no cellgraph door
between steps. Its arena nodes are **settlements**, one body's facts settled
under one binding and found by their parent and key, and **compositions**,
the facts of code composed at run time, children of the root found by syntax
digest. The root is the frozen load itself and no slot. Each arena node rests
in a slot with a generation, bumped at its eviction, so an `ArenaRef` held
past it names nothing rather than the slot's next tenant. Its facts are a
`self_cell` over its own bump and an `Rc` of each arena node its records read
— its parent, the composition its chain rests on, and each composition it
pins — and a step reads them borrowed from the arena for the step alone.

A cell is born under an arena node, which the drain resolves at its creation
from how it was asked for ([seek.rs](seek.rs)), and borrows it for its life,
tenants included:

- `spawn`, `hand_off` and `tail` ask for an evaluation, a block or a hand-off,
  born under what its asker runs under.
- `spawn_seeking` and `tail_seeking` ask for a call's frame, handing a `Seek`:
  the arena node the callee was born under and, where its body keys a
  settlement on content the load names, the binding's image beside the
  request by value (`Sought`): the key, what a miss settles and the types the
  binding carries. A live hit is stamped. A miss settles the body there
  and then, through the program's `Settles` door, relaying the image into the
  agent tier under the root, and the frame is born under the new settlement.
  A `Seek` that `hands_back` — a frame's whose callee's body can birth a
  callable — has the drain keep the request, beside its binding's image, in
  the frame's continuation where the frame is born under that settlement, by
  a hit, a miss, a replay's last level or a pinning seek, and its first step
  takes it (`sought`, a `Requested`) to relay the image into its own region
  and lay the frame's level there
  ([the program](../program/README.md#settlements-and-activations)), so the
  level's types resolve for a callable born under it wherever its asker was;
  it is the scheduler's runtime state, never written into a region. A seek with no
  key runs the frame under the callee's arena node; one whose
  parent is gone runs it under the root, where its first step replays its
  callee's stamp (`replay`): each level found or settled under the one
  before, borrowed until the frame borrows its own, so the frame is born
  warm.
- `tail_under` hands the `ArenaRef` its frame ran under, for a hop of the same
  callee: the successor borrows that settlement with no lookup, stamping it,
  or runs under the root where it is no longer live.
- `spawn_composing` asks for a cell running code composed at run time, under
  its composition, found by syntax digest. The books hold digests alone, so
  on a miss the cell is born under the root and its first step copies the
  code's syntax and asks again (`bounce`), and the drain builds the
  composition over the copy. A miss under a composition binding a module of
  other composed code pins that code's composition; where it is gone, the
  frame's first step copies the module's syntax and asks again (`rebuild`).

So no cell parks for its arena node. A hand-off successor borrows before its
predecessor's release, and every retire releases its cell's borrow.

**Held and evicted.** An arena node is **held** while a run cell borrows it,
while a held arena node lies below it, or while a held settlement pins it; the
held total moves only where a count turns, so a borrow costs the depth of its
chain. One **budget** bounds the bytes of the arena nodes no frame holds
(`BuildBudget`, 4 MiB by default): after a miss, while the unheld bytes exceed
it, the unheld arena node with no child, no pinner and the oldest use is
evicted, so eviction takes leaves first, and an evicted settlement releases
its pin before the composition it pinned is a candidate. What frames hold is
linear in the frame stack's distinct arena nodes and the compositions they
pin, whatever the nesting depth.

## Memory

The slab holds roots and nothing else: the tree pool takes no cap, so a call
subtree deeper than any slab cap runs on tree cells without touching the matrix.
The matrix width is therefore one word — `WIDTH` in
[substrate.rs](../memory/substrate.rs) is `1`, sixty-four slab slots.

The view's heap is the ready stack and the request buffer. Both are amortized,
neither grows per step once warm, and the stack's depth is the unborn siblings
along the current path. The graph's heap beyond its cells is the build
arena: its slots and maps, sized by the live arena nodes, and each arena
node's bump, bounded by the budget where no frame holds it. Everything a computation keeps across a park is in its
cell: the continuation in storage, the in-progress state and the receipt run in
the scratch habitat, handed back whole at the first step end that leaves
nothing naming it.

## The import rule

Outside doc comments this module names `crate::knot`, `crate::memory`,
`crate::scope`, `crate::type_lattice` and `crate::values`, and nothing else in
the crate — its tests included, which may name the crate's test scaffolding
besides. It does not name `parse`, `elaborate`, `symbols` or `program`. It
names the lattice for the frozen load, the agent tier and a branch alone, the
tiers a graph holds and hands every step and crossing; `scope` for the header a graph keeps
per habitat half (`RegionHeader`), the facts a step's door reads and an arena
node holds, and the decision memo; and `values` for the type chain those doors
build and the arena's key, image, budget and handle (`SettlementKey`,
`BindingImage`, `BuildBudget`, `ArenaRef`). `scope`
names `values` and is named by `knot`, so the edge forms no cycle, and neither
names this module. `cellgraph` is reached
only through `memory`, and `cellgraph` itself depends on neither this module
nor koan. `tests/boundary.rs` reads the source to hold the rule there; the
ready stack and the request buffer are the owning heap types it blanks,
because each is the scheduler's own runtime state and never a value in a
region, beside the replay's request list. It also holds `Step`'s public
signatures free of every handle, every module but this one and `values` from
opening an `ArenaRef`, and the drain from opening a cellgraph door between
steps.

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
- a ten-thousand-hop loop headed by a tree child and by a tenant, with three
  cells live at the peak and no more heap than the same loop a hundred hops
  long;
- a subtree two hundred levels deep on a slab of one;
- what a birth costs a branch: a child born under a spawner holding any number
  of types holds only the one it reads, the deepest level of a recursion
  minting at every level holds as much at depth sixty-four as at eight, a
  tenant's hop reaches what its host read through its link, each way a deep
  cell hands a value on — a hop and a hand-off from a tree cell and a tenant,
  and a kept result — leaves the landing cell's branch the same at depth four
  and sixteen, and a cycle of hand-offs and hops checks as many reach steps
  however deep its host; and the caching chains: a `Shares` chain laying
  in its host's scratch and leaving its region, and a held `Fresh` child
  reading its region and its own scratch, with no agent chain
  ([tests/branches.rs](tests/branches.rs));
- a root work that leaves a birth a later root work resumes from, at each
  placement, and a cell that reports to somebody refused on `leave`;
- two schedulers beside each other sharing nothing — each test seeds its own
  type tiers in a local bump, released with the test — two views over one graph in
  turn, and a view dropped mid-work leaving a graph its root's release does not
  empty;
- the round trip a continuation makes between `'graph` and a step's `'here`,
  through the raw slot doors a step never reaches;
- the load door standing one slab root after the load, and handing a
  refusal back ([tests/load.rs](tests/load.rs));
- the arena alone: a stale handle naming nothing once its slot is reused,
  eviction taking the least recently used unheld leaf first, a borrowed
  settlement and every ancestor of it staying through eviction, unheld bytes
  within the budget after every miss, a parent evicted only after its last
  child, and a child settlement and a pinned composition each releasing what
  it holds last ([tests/arena.rs](tests/arena.rs));
- a missed settlement built once and found warm by the next birth, two
  bindings giving two under one parent, a settlement nesting under the frame
  that seeks beneath it, a miss past the budget evicting the least recently
  used unheld settlement, a parent going after its child, a loop of hops
  holding its settlement through every miss, a call keying nothing running
  under its callee's arena node, a hand-off successor holding its frame's
  settlement until its own release, a tenant's count running from its
  creation to its release, every run cell born under a settlement carrying
  its reference, and a missed settlement relaying the image of a binding its
  asker minted, read by a frame after the asker is gone
  ([tests/seek.rs](tests/seek.rs)).

A native step is a bare `fn` and carries no closure state, so what a step
observes it records in `tests/native.rs` for the test around it to read back.

## Open work

- [Setting the build budget](../../roadmap/gradual-build/bound-settings.md)
  — the budget as a setting a user names.
- [Agents](../../roadmap/rewrite/agents.md) — an arena per agent.

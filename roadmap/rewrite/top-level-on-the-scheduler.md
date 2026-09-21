# The top level on the scheduler

What turns a koan program into work for the drain: the root cell a top-level
binding lives in, the components its statements are, and the tie that binds them.

**Problem.** The [scheduler](../../src/scheduler/README.md) drives native steps —
a function pointer over a state value — and nothing turns a program into those.
The pieces it would compose already ship and have never been wired to each
other: [shapes](../../src/scope/README.md) resolve a body's names to slot
indices and condense its bindings into components, [the
tie](../../src/knot/README.md#the-tie) births a deferred-only component as
one knot, and [values](../../src/values/README.md) prices every crossing. What is
missing between them is where a top-level binding lives, which cell a statement
is, how a component's dependencies reach the drain's submission table, where the
builtin table a shape resolves its names against rests once a program runs, and
where the placement bit for a koan function comes from. Two of those layers record the
gap as open work of their own: `scope` does not say which habitat each tier of an
activation is laid down in, and `knot` does not say who evaluates the eager
part a refused tie names.

**Acceptance criteria.**

- A top-level binding lives in the region of a **root** slab cell that is
  created before the first statement, released after the last, and never
  entered. The top level's slot array lives in program storage at `'graph` and a
  bound slot holds a dormant carrier homed in the root, so a statement cell
  claims, binds and reads a slot with no door.
- The builtin table rests in program storage at `'graph`, laid down by the
  constructor a region's table is, through the writer of a store `cellgraph`
  owns outside the graph. An activation at any brand names it with no door,
  so a call pays nothing to reach a builtin.
- The top level's statements are tree cells under the root, so a binder builds
  its value in the root's region by an upward crossing the verdict prices, a
  reader redeems it entitled by root identity with no hold of its own, and the
  slab holds the root and nothing else. A binder's region splices into the root
  at retirement when the verdict pinned it and is reclaimed when it copied, and
  its intermediates go to its scratch habitat and are freed at retirement either
  way.
- A program with more top-level statements than the slab cap runs to
  completion, because the tree pool has no cap and nothing a running statement
  asks for takes a slab slot: its call subtree is tree cells and tenants under
  its own root, a stream at rest is a value
  ([yielding-iterators.md](yielding-iterators.md)), a module activation is data,
  and a top-level binding lives in the root.
- A deferred-only component of a body's bindings
  ([src/scope/README.md](../../src/scope/README.md#visibility)) is one unit of
  work: one cell claims every member's slot as its first act and binds every
  member's slot from the knot the tie hands back.
- A component's submission carries the count of lower components it reads,
  taken from the shape's reference graph in condensation order, so a refused tie
  naming a pending binder
  ([src/knot/README.md](../../src/knot/README.md#the-tie)) is a
  scheduler bug rather than a wait. A statement containing `EVAL`, whose free
  names no shape can enumerate, counts every binder declared before its position.
- Two units whose counts reach zero in the same round are launched in the order
  their statements are written, so a run is deterministic and a program whose
  statements have effects and no data dependency between them takes source
  order.
- The statements of a called body reach the submission table from the step that
  activates the body, which asks for a unit the way it asks for a child: the
  drain performs every birth, and no step names a place that is not its own.
- A cell builds its result in the region its spawner named rather than in its
  consumer's, so a called body's result outlives the frame that ran it and the
  value a top-level call binds is built in the root from the start.
- A refused tie naming an eager part of a data member becomes a sub-dispatch
  whose value the step supplies to the tie by site when it re-runs, and a step
  refused on several parts at once wakes once, when the last receipt slot fills.
- Every statement cell of a called body is a tenant of its frame, so binding a
  claimed slot is a plain write at `'here` and a reader of a bound slot reads it
  where it lies; the value never travels to the reader.
- The [scheduler](../../src/scheduler/README.md) names no step of koan's: it
  takes the step state as one bundle parameter, as it takes delivery, and its
  continuation is one shape with no arm to add. Koan's steps and the state they
  run over are one module above it, `program`, whose design doc is the `README.md` in its
  source directory and whose top-of-file comment links it.
- The program record carries one `evaluate` function that turns a node and the
  environment it is read in into a child the drain can create, and the component
  step spawns an eager part through it — so evaluating an expression stays
  [dispatch](dispatch.md)'s and nothing below it names an expression form.
- A read of a name goes through one interface the activation of a called body
  and the top level's staged reads both answer, so
  [the tie](../../src/knot/README.md#the-tie) and
  [elaboration](../../src/elaborate/README.md) name no habitat.
- The placement bit a spawn carries is derived: a builtin declares it, and a
  user function derives it from its return type, a flat return being one that
  cannot share. A `MODULE` or `GROUP` activation takes no cell of its own — it
  is data like any other value, placed by the ordinary crossing verdict in the
  region of whatever keeps it.
- A whole koan program — top-level bindings, calls, a recursive call deeper than
  the slab cap, and a deferred-only component with a pending sibling and an eager
  part — runs to completion under the drain, and the run's Miri slate is clean.

**Directions.**

- *Where the top level's bindings live — decided.* In the region of a root slab
  cell, storage-only and never entered, that outlives every statement. Only a
  cell can hold, and a hold is what lets a binder's region splice at retirement
  rather than reclaim; program storage
  ([src/scope/README.md](../../src/scope/README.md#three-tiers)) is no cell, and
  a `'graph` borrow cannot embed a `'here` one, so a top-level value kept there
  could only be a deep copy. The slot array itself does live in program storage:
  `'graph` is nameable from every step, so a claim and a bind are plain `Cell`
  writes from any statement cell, and what a bound slot holds is a dormant
  carrier, which carries no brand. A `SlotArray` is invariant in its brand, so
  it is instantiated at two habitats — `'graph` at the top level holding
  carriers, `'here` in a frame holding values — and they are distinct types.
- *Where the builtin table lives — decided.* In program storage, in a store
  `cellgraph` owns outside the graph and hands a `Writer` over, so the embedder
  never names the bump underneath. Program storage holds it beside the bump
  the AST is parsed into. A shape resolves a builtin name to an index into the table, so the
  table has to exist before the program's shape does and before any cell. A
  `'graph` borrow is nameable from every step and embeds in any region at no
  price, and the table is covariant in its cell brand, so it shortens to any
  activation's. The store is sound for the reason a `'graph` borrow is no
  operand: the storage outlives the graph, which prices, pins and reclaims none
  of it. The alternative, a table in the root's region written by the root's
  first tree child, costs a redeem and a pinned placement in every cell that
  owns an environment — one per call in a region of its own.
- *Statements are tree children of the root, not slab cells — decided.* The tree
  pool takes no cap, which is what lets every statement of a body have its slot
  claimed before any of them runs; and a tree cell's redeem entitlement is root
  identity, so a statement reads a top-level binding homed in its own root with
  no hold. The alternative, a tenant of the root, would make a bind free but
  leave nothing a statement allocates ever reclaimable. Because the pool has no
  cap there is no admission policy to write: no marker into the AST, no refused
  create, and no deadlock argument — position order is still a topological order
  of the reference graph, but nothing turns on it beyond the submission counts.
- *What a statement of a called body is — decided.* A tenant of its frame, so
  binding a claimed slot is a plain write at `'here` and a reader reads the slot
  where it lies.
- *Where the destination of a top-level call is — decided.* The destination
  forwards: a call whose result is bound for the root builds it there, and a
  *shares* call passes that destination to the argument it embeds, so in
  `x = cons(1, f(z))` at the top level `f` builds its result in the root's
  region for `cons` to embed. A *shares* call made at the top level has no frame
  to be a tenant of and is a tenant of its statement's tree cell.
- *Where the placement bit comes from — open.* First cut: builtins declare the
  bit and a user function derives it from its return type, since a flat return
  cannot share. Open beyond the first cut, in precedence order over the derived
  bit: a programmer annotation with no semantic effect, since the cost of a
  wrong guess scales with data size only a programmer can predict; and a runtime
  measurement of copied and retained bytes per function. *Recommended:* ship the
  first cut and leave both extensions to the item that needs them.
- *How the drain learns a component's dependencies — decided.* From the shape's
  reference graph, condensed. The shape emits its statements' units in an order
  where each follows every unit it reads and two independent units come out as
  they are written, so a unit's count is the number of distinct lower units its
  members read, the edges are the reads themselves, and the drain launches the
  lowest-numbered ready unit first. This is what makes a refused tie naming a
  pending binder unreachable, and it is why the
  [scheduler](../../src/scheduler/README.md#the-two-ways-a-cell-waits) needs no
  park on a slot.
- *Where a body's statements come from — decided.* From the shape that owns
  them, since the [shape builder](../../src/scope/README.md#operator-groups)
  rewrites an operator run
  where the shape is built and every part address a shape records lies inside
  the statements it owns.
- *Where koan's steps live — decided.* One module above the
  [scheduler](../../src/scheduler/README.md), which takes the step state as a
  bundle parameter beside its delivery bundle. A step's state has to live with
  the step: the alternative — the scheduler declaring an arm per step of every
  layer above it — makes the drain the declaration site for dispatch, modules
  and iterators in turn, and widens its import rule each time.
- *How a top-level binding reaches a reader — decided.* Staged: the unit
  redeems the dormant carrier each slot it reads holds and crosses it into its
  own cell, where the price is zero because the value is already homed in the
  reader's root, and hands the tie a reader over the staged run. The
  alternative, an activation over the program's slots materialized in the
  statement's own region, costs the whole program's slot count per statement and
  inflates the region enough to flip the binder's upward crossing to a copy.
- *How effects are ordered — deferred.* Launch order gives source order except
  where a unit's dependencies are met late, and no rule says more. An effects
  item chains the statements that can have effects, which is the only ordering
  that survives a park; until then the gap is recorded under
  [unplanned work](README.md#unplanned-work).
- *What the `Pending` refusal becomes — open.* `function::Untieable::Pending`
  and `scope::Binding::Pending` both carry a producer `CellHandle` that nothing
  reads once dependencies are wired statically, and `SlotArray`'s `Claimed`
  payload carries it too. *Recommended:* keep all three as the diagnostic they
  become — a scheduler bug names the binder it found in flight — and drop the
  handle only if it proves unreachable in practice.

## Dependencies

**Requires:** none — every value a top-level statement places ships.

**Unblocks:**

- [Dispatch](dispatch.md) — running a program needs the scheduler that drives it.
- [Yielding iterators](yielding-iterators.md) — a flat consumer loop is its tail call.
- [The AST in `cellgraph` storage](ast-in-graph-storage.md) — it moves the AST into the store this item adds.

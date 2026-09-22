# The top level on the scheduler

What turns a koan program into work for the drain: the root a top-level binding
lives in, the body runner that performs a body's units, and the tie that binds
them.

**Problem.** The [scheduler](../../src/scheduler/README.md) drives native steps —
a function pointer over a state value — and nothing turns a program into those.
The pieces it would compose already ship and have never been wired to each
other: [shapes](../../src/scope/README.md) resolve a body's names to slot
indices and condense its bindings into components, [the
tie](../../src/knot/README.md#the-tie) births a deferred-only component as
one knot, and [values](../../src/values/README.md) prices every crossing. What is
missing between them is where a top-level binding lives, which cell performs a
statement, in what order a body's units are performed, where the builtin table
a shape resolves its names against rests once a program runs, and where the
hints a spawn carries come from for a koan function. Two of those layers record
the gap as open work of their own: `scope` does not say which habitat each tier
of an activation is laid down in, and `knot` does not say who evaluates the
eager part a refused tie names.

**Acceptance criteria.**

- A top-level binding lives in the region of a **root** slab cell that is
  created before the first statement, released after the last, and never
  entered. The top level's activation is laid down there by the **body runner**,
  a tenant of the root born as the root work of each call, so a top-level bind
  is a plain write at `'here` and the top level is a frame like any other.
- The builtin table rests in program storage at `'graph`, laid down by the
  constructor a region's table is, through the writer of a store `cellgraph`
  owns outside the graph. An activation at any brand names it with no door,
  so a call pays nothing to reach a builtin.
- One body runner performs the top level and every called body: one step
  function whose state is the activation, the next unit and the stage it parked
  at. It claims nothing ahead, ties and binds each unit itself, and no unit has
  a cell of its own. In a called body it runs in the frame's own cell.
- The shape emits a body's units in an order where each follows every unit it
  reads and two independent units come out as they are written, and the body
  runner performs them in that order, so a refused tie naming a pending binder
  ([src/knot/README.md](../../src/knot/README.md#the-tie)) is a scheduler bug
  rather than a wait. A statement containing `EVAL`, whose free names no shape
  can enumerate, follows every unit that binds a name declared before its
  position.
- A program whose statements have effects and no data dependency between them
  takes source order, and the effects of one statement's evaluation never
  interleave with the next's.
- The only children a body runner asks for are evaluations, through the
  program record's one `evaluate` function, which turns a node and the
  environment it is read in into a child the drain can create — so evaluating
  an expression stays [dispatch](dispatch.md)'s and nothing below it names an
  expression form.
- The evaluation of a top-level statement's eager part is a `Fresh` tree cell
  under the root with the root as its result's home, so the value is built in
  the root's region from the start, everything else the evaluation allocates is
  reclaimed at its retirement, and the slab holds the root and nothing else. A
  result the evaluation built in its own region crosses into the root at the
  verdict's price: its region splices into the root at retirement when the
  verdict pinned it and is reclaimed when it copied.
- A program with more top-level statements than the slab cap runs to
  completion, because nothing a running statement asks for takes a slab slot:
  its call subtree is tree cells and tenants under its own root, a stream at
  rest is a value ([yielding-iterators.md](yielding-iterators.md)), a module
  activation is data, and a top-level binding lives in the root.
- A deferred-only component of a body's bindings
  ([src/scope/README.md](../../src/scope/README.md#visibility)) is one unit: the
  body runner ties it once and binds every member's slot from the knot the tie
  hands back.
- An evaluation is asked for with the `Use` its result is bound for — `Keeps`
  for a binding, `Forwards` for a body's last value, `Reads` for a condition or
  a discarded statement — so a called body's result outlives the frame that ran
  it and the value a top-level call binds is built in the root from the start.
- A refused tie naming an eager part of a data member becomes an evaluation
  whose value the body runner supplies to the tie by site when it ties again,
  and a runner refused on several parts at once wakes once, when the last
  receipt slot fills.
- An evaluation asked for at `Shares` in a called body is a tenant of the frame,
  so what it builds is at the frame's `'here`, binding a slot is a plain write,
  and a reader of a bound slot reads it where it lies; the value never travels
  to the reader.
- An evaluation reads names through a read-only view of the activation it was
  asked from — the root's at the top level, the frame's in a called body —
  which has no door that claims or binds a slot, pinned by a `compile_fail`
  doctest. [The tie](../../src/knot/README.md#the-tie) and
  [elaboration](../../src/elaborate/README.md) read one activation type at both
  levels and name no habitat.
- Koan's steps and the state they run over are one module above the
  [scheduler](../../src/scheduler/README.md), `program`, which supplies the
  scheduler's step bundle, whose design doc is the `README.md` in its source
  directory and whose top-of-file comment links it.
- The placement bit a spawn carries is derived: a builtin declares it, and a
  user function derives it from its return type, a flat return being one that
  cannot share. A `MODULE` or `GROUP` activation takes no cell of its own — it
  is data like any other value, placed by the ordinary crossing verdict in the
  region of whatever keeps it.
- [`CellSubstrate`](../../src/program/README.md)'s load builds the builtin table, the
  program's shape, the root and the `Program` record in its builder, keeps the
  record beside the graph, and returns a `Result` carrying the parse error or
  `ShapeError` that stopped it.
- A test loads two programs through a helper function that returns each
  `CellSubstrate`, moves both into a `Vec`, runs each, and reads a top-level
  binding after the drain in a separate call from the one that ran it, through
  a second root work born under the same root.
- A whole koan program — top-level bindings, calls, a recursive call deeper than
  the slab cap, and a deferred-only component with a pending sibling and an eager
  part — runs to completion under the drain, and the run's Miri slate is clean.

**Directions.**

- *Where the top level's bindings live — decided.* In an ordinary activation in
  the region of a root slab cell, storage-only and never entered, that outlives
  every statement. The body runner is a tenant of the root, so its `'here` is
  the root's region, a bind is a plain write, and the drain creates and retires
  every cell it runs. A root that was itself the runner would be a cell the
  drain must not retire, and a later call over it would have to re-arm a
  finished cell. A slot array in program storage holding dormant carriers would
  be a second habitat behind a second read interface, would cost a redeem and a
  crossing per read, and would put run-time state in the tier that is never
  released.
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
- *What performs a statement — decided.* The body runner, inline, at both
  levels. A cell per unit would cost a create, an enter, a release and a
  continuation store per statement per call, and a receipt slot per statement
  per body, for statements that mostly never park; inline costs the unit's
  stage in the runner's state. The tie and the bind are the same code at the top
  level and in a frame, and the two differ only in the placement of the
  evaluations they ask for.
- *What a top-level statement's evaluation is — decided.* A `Fresh` tree child
  of the root. The root's region lives as long as the program, so anything the
  runner wrote there beyond a binding would never be reclaimed: the runner
  writes bindings and nothing else, and an evaluation's arguments, records and
  intermediates rest in a region of its own that dies with it. The tree pool
  takes no cap, so nothing here needs an admission policy. In a called body the
  frame dies soon enough, and an evaluation takes the placement its callee
  declares.
- *Where the destination of a top-level call is — decided.* The home forwards: a
  call whose result is bound for the root builds it there, and a *shares* call
  asks for the argument it embeds with `Forwards`, so in `x = cons(1, f(z))` at
  the top level `f` builds its result in the root's region for `cons` to embed.
  A *shares* call made at the top level has no frame to be a tenant of and is a
  tenant of its statement's evaluation cell.
- *Where the placement bit comes from — open.* First cut: builtins declare the
  bit and a user function derives it from its return type, since a flat return
  cannot share. Open beyond the first cut, in precedence order over the derived
  bit: a programmer annotation with no semantic effect, since the cost of a
  wrong guess scales with data size only a programmer can predict; and a runtime
  measurement of copied and retained bytes per function. *Recommended:* ship the
  first cut and leave both extensions to the item that needs them.
- *Where a body's order comes from — decided.* From the shape's reference graph,
  condensed. The shape emits its statements' units in an order where each
  follows every unit it reads and two independent units come out as they are
  written, and the runner performs them in that order over a depth-first drain,
  which finishes each before the next begins. An outside event reaches a program
  only through a monad, which a shape sees, so no unit becomes ready in any
  order but the shape's. This is what makes a refused tie naming a pending
  binder unreachable, and it is why the
  [scheduler](../../src/scheduler/README.md#how-a-cell-waits) keeps no
  dependency count and no park on a slot.
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
- *How a binding reaches an evaluation — decided.* By reference: the evaluation
  is handed a read-only view of the activation it was asked from as part of its
  state, which arrives pinned, since the activation's home is above the
  evaluation and the crossing prices zero. Staging each read into the
  evaluation's own region costs a redeem and a crossing per read and weighs the
  region enough to flip an upward crossing to a copy. The view is read-only
  because the activation's slots are `Cell`s of a region that outlives the
  evaluation, and it is a covariant type of its own: a state crosses only as a
  [`Covariant`](../../cellgraph/src/reattach.rs) family, and an activation,
  whose `Cell`s hold borrows at its brand, is invariant and never crosses.
- *How effects are ordered — deferred.* Unit order is source order except where
  a forward reference moves a binder ahead of its reader, a reordering the shape
  makes and a program can predict, and no rule says more. An effects item gives
  the statements that can have effects their own order; until then the gap is
  recorded under [unplanned work](README.md#unplanned-work).
- *What the `Pending` refusal becomes — open.* With a body's order fixed by its
  shape nothing claims a slot ahead of binding it, so `SlotArray`'s `Claimed`
  state, `scope::Binding::Pending` and `function::Untieable::Pending` are never
  observed, and the producer `CellHandle` each carries is read by nothing.
  Either keep all three as the diagnostic they become — a scheduler bug names
  the binder it found in flight — or make a slot two-state and rewrite
  [placeholders and writes](../../src/scope/README.md#placeholders-and-writes)
  to say a slot is empty until its unit's turn. *Recommended:* make it
  two-state, since a state nothing can reach is a protocol nothing tests.

## Dependencies

**Requires:** none — the scheduler it runs on and the substrate beneath it ship.

**Unblocks:**

- [Dispatch](dispatch.md) — running a program needs the scheduler that drives it.
- [Yielding iterators](yielding-iterators.md) — a flat consumer loop is its tail call.
- [The AST in `cellgraph` storage](ast-in-graph-storage.md) — it moves the AST into the store this item adds.

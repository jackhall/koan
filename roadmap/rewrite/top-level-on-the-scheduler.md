# The top level on the scheduler

What turns a koan program into work for the drain: the root cell a top-level
binding lives in, the components its statements are, and the tie that binds them.

**Problem.** The [scheduler](../../src/scheduler/README.md) drives native steps —
a function pointer over a state value — and nothing turns a program into those.
The pieces it would compose already ship and have never been wired to each
other: [shapes](../../src/scope/README.md) resolve a body's names to slot
indices and condense its bindings into components, [the
tie](../../src/function/README.md#the-tie) births a deferred-only component as
one knot, and [values](../../src/values/README.md) prices every crossing. What is
missing between them is where a top-level binding lives, which cell a statement
is, how a component's dependencies reach the drain's submission table, and where
the placement bit for a koan function comes from. Two of those layers record the
gap as open work of their own: `scope` does not say which habitat each tier of an
activation is laid down in, and `function` does not say who evaluates the eager
part a refused tie names.

**Acceptance criteria.**

- A top-level binding lives in the region of a **root** slab cell that is
  created before the first statement, released after the last, and never
  entered. The top level's slot array lives in program storage at `'graph` and a
  bound slot holds a dormant carrier homed in the root, so a statement cell
  claims, binds and reads a slot with no door.
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
  work: one cell claims every member's slot at submission and binds every
  member's slot from the knot the tie hands back.
- A component's submission carries the count of lower components it reads,
  taken from the shape's reference graph in condensation order, so a refused tie
  naming a pending binder
  ([src/function/README.md](../../src/function/README.md#the-tie)) is a
  scheduler bug rather than a wait. A statement containing `EVAL`, whose free
  names no shape can enumerate, counts every binder declared before its position.
- A refused tie naming an eager part of a data member becomes a sub-dispatch
  whose value the step supplies to the tie by site when it re-runs, and a step
  refused on several parts at once wakes once, when the last receipt slot fills.
- Every statement cell of a called body is a tenant of its frame, so binding a
  claimed slot is a plain write at `'here` and a reader of a bound slot reads it
  where it lies; the value never travels to the reader.
- The scheduler takes one `evaluate` function at construction that turns a part
  and its activation into a continuation, and the component step spawns an eager
  part through it — so evaluating an expression stays [dispatch](dispatch.md)'s
  and the scheduler names no expression form.
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
  reference graph, condensed. `Shape::components()` emits in reverse topological
  order on the condensation, so a component's count is the number of distinct
  lower components its members read, and the edges are the reads themselves.
  This is what makes a refused tie naming a pending binder unreachable, and it
  is why the [scheduler](../../src/scheduler/README.md#the-two-ways-a-cell-waits)
  needs no park on a slot.
- *What the `Pending` refusal becomes — open.* `function::Untieable::Pending`
  and `scope::Binding::Pending` both carry a producer `CellHandle` that nothing
  reads once dependencies are wired statically, and `SlotArray`'s `Claimed`
  payload carries it too. *Recommended:* keep all three as the diagnostic they
  become — a scheduler bug names the binder it found in flight — and drop the
  handle only if it proves unreachable in practice.

## Dependencies

**Requires:**


**Unblocks:**

- [Dispatch](dispatch.md) — running a program needs the scheduler that drives it.
- [Yielding iterators](yielding-iterators.md) — a flat consumer loop is its tail call.

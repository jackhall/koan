# Tree cells

**Problem.** The live slab is bounded and `create` refuses at the cap
([design/liveness-matrix.md § Bounding the two
tiers](../design/liveness-matrix.md#bounding-the-two-tiers)). A non-tail
recursion holds one pending cell per level, each awaiting its child, so none of
the admission answers that section lists applies: the pending parents *are* the
population, they cannot drain, and at a depth equal to the cap admission
deadlocks instead of degrading. Every cell today takes a slab slot because the
matrix is the only liveness habitat there is.

A call subtree needs none of what the matrix provides. Two facts already in
force make its liveness a stack discipline. A parent outlives its children: a
body waits on its leading statements before its tail runs
([design/execution/calls-and-values.md](../../design/execution/calls-and-values.md)),
and a reinstall retires an incarnation only after they resolve. And a cell's
values leave it only through its terminal, whose destinations all lie on one
ancestor chain: koan mints one source edge per producer, deps inherit that
destination, and the only second destination the alias splice can add belongs
to a forwarder that lexically sits under the binding's frame and whose parked
target is never its own child. So before the root's terminal nothing outside
the subtree can pin a cell inside it, and inside it every hold points up the
chain. A hold that always points at a live ancestor needs no bit and no count.

**Acceptance criteria.**

- A third cell kind, the **tree cell**: it owns its region outright, occupies
  no slab slot, and has no row, no column, no holder count, and no id — no
  mask ever names a tree region. Its handle is an index plus generation in a
  growable pool; the cell records its root slab handle and its parent tree
  handle, and nothing else about liveness.
- A tree cell's placement doors mint into its **root's** pin row and sealed
  set. Every slab or sealed region a tree value can reach arrived through the
  root's chain, the root's step, or an ancestor's inputs, so the root's rows
  already name it; the mint keeps the ordinary door and no new hold relation.
- Release is a two-way choice from what the delivery already carries: if the
  terminal is not homed in the dying region, the region reclaims; if it is,
  the bump splices into the terminal's canonical destination, O(1), with a
  priced copy-out into that destination as the compaction alternative. No
  count arithmetic and no seal transition runs at a tree cell's death.
- The delivery walk adopts a tree terminal once, into its canonical
  destination — the producer's own source edge's region, the shallowest on
  the chain — and the deeper destination buckets read that resident by
  reference, the read a parked consumer already takes.
- `redeem` in a tree cell is O(1): the value's home is the cell itself, a live
  tree cell under the same root (generation-checked), or a region the root's
  rows name. A sibling's value is never readable until the sibling has died
  and absorbed, so a live non-ancestor home cannot arise.
- koan's kind rule is an admission decision at creation: a cell whose source
  edge is destined at its creator's region, or at a tree cell under the same
  root, is a tree cell. Top-level statements, yielding producers, and any cell
  an outside consumer can pin while it lives stay slab cells. A non-tail
  recursion deeper than the slab cap runs to completion; the shape that
  refuses admission today is the regression test.
- Scheduling is untouched: slots, deps, wake and notify, the priority bands,
  the drain, and reinstall behave identically, and the existing slot-count and
  TCO assertions hold unchanged.
- The Miri slate gains: a splice into a destination under a live borrow into
  the moved bump, a redeem of a value whose home was absorbed, and a reinstall
  inside a tree.
- The **group sealing** gap retires with the item: a subtree's internal holds
  never exist, so there is no chain of records to collapse.

**Directions.**

- *Kind selection — decided.* Dynamic, at admission, from the source edge's
  destination. Static knowledge — a call site resolved to its own function
  under [declaration windows](../../roadmap/metaprogramming/declaration-windows-gate-dispatch.md),
  or a return type that can carry no reach — is a later refinement of the
  choice, not the mechanism: it cannot see through a closure or a functor.
- *Chain property — decided premise, to be tested.* The one route the
  argument above does not close by construction is a closure returned out of
  a body and called from outside while a binding it forwards to is still
  pending. Bodies waiting on their leading statements rules it out today
  ([design/lazy-closures.md](../../design/lazy-closures.md): capture by
  reference, severance by copy at the escape seam); the item ships a test that
  pins the property so a later change to body ordering trips it.
- *Absorb versus copy — decided as pricing.* The splice retains the dying
  cell's whole bump, garbage included; the copy compacts and costs the value's
  size. The existing consolidation pricing decides, per release.
- *Pool residence — open.* (a) A third pool inside `cellgraph`, beside the
  slab and the sealed tier, with its own handle type; (b) the tree cell rides
  on `workgraph`'s node store, since it carries nothing but a region and two
  handles. (a) keeps the tier vocabulary in one crate; (b) avoids a second
  index for a cell the scheduler already names.
- *Home after absorption — open.* After a splice, values physically in the
  destination still record the dead tree cell as home. The terminal itself
  arrives as a fresh resident at the destination with derived reach, and
  interior reads re-anchor at the reading borrow, as sealed reads do. Whether
  that is the whole answer or a tree-tier accessor is needed.
- *Out-of-tree deliveries — settled.* A yield delivers mid-life to an outside
  consumer, so a yielding producer is a slab cell; that answers the
  dormant-slot habitat question in
  [yielding-iterators.md](../../roadmap/foundation/yielding-iterators.md).
  Effects flow through terminals — a builtin names the effect on the carrier
  it returns ([design/effects.md](../../design/effects.md)) — and need no
  exception.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.

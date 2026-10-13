# The host's scratch as a type cache

A sealed body's minted types routed per site at the load: content the load
can compute into the root, content a loop mints alike each hop into the
scratch branch of the host whose evaluation asked for the call, where it
lives from the ask to the delivery and is warm across every frame born under
that park, and content that differs per hop into the frame's own region under
`Fresh`, where each hop recycles it.

**Problem.** A `Shares` callee lays its bytes in its host's region and
interns the types it builds into the host's
[branch](../../lattice/src/types/identity.md#storage-a-root-and-its-branches).
A body that views a module and returns a string leaves nothing reachable when
it returns, yet every carrier it minted, and every type built over one, stays
in the host's branch until the host finishes: a host that calls such a body
once per element of a list of modules grows its branch per element, and a
[`Shares` tail hop](../rewrite/README.md#unplanned-work) leaves its
predecessor's types with the host the same way. A `Fresh` callee has the
opposite cost: its region dies at its return, so a `Fresh` tail loop that
views the same module each iteration lays the same carrier and template every
hop, and recomputes every verdict over them. And a parked cell's receipt run
is laid down fresh in its scratch habitat per registration
([the cell](../../cellgraph/README.md#the-cell)), so a loop that parks once
per iteration lays one run per iteration into the scratch its types would
share. Over the tutorial's runnable programs no sealed body mints, so none
caches (`classes_over_the_tutorial` in
[the class tests](../../src/dispatch/tests/classes.rs)).

**Acceptance criteria.**

- A sealed body reached by a call or an `EVAL` interns each mint site as the
  load classed it ([where each body interns](../../design/type-tiers.md#where-each-body-interns)):
  a closed site hits the root; an invariant site interns into the warm tier,
  the scratch branch its park holds; a varying site into the cold tier, the
  frame's own scratch under `Fresh`, held for the frame's life and dropped
  with it, and the host's scratch under `Shares`; a composite site follows its
  children. A test calls a sealed `Shares` body repeatedly
  in non-tail position with new content each call and holds the host's region
  node count constant across calls.
- Every frame born under a park has that park's scratch branch as its warm
  tier: a tenant its host's, a `Fresh` callee the branch its asker's park
  holds, lent by a cellgraph door, and a hop's successor its predecessor's,
  lent or own, whatever either's placement. A cell lent a branch names that
  scratch until it departs. A test runs a `Fresh` tail loop over an invariant
  open module and holds the scratch's node count constant across iterations;
  a second runs a `Fresh` tail loop over varying content and holds the
  lattice flat across iterations; a third runs a tail chain across two bodies
  of different placement over invariant content the same way; each sees the
  branch reset once the loop's value is delivered.
- A lent branch is laid through the root and itself alone, so a node over a
  child in the writing cell's own region misses at the lay, pinned by a test.
  A `Fresh` caching body reads through a chain whose own tier is its scratch,
  reading the lent branch and, through a link, its region. A test finds no
  region node referencing a scratch node and no lent node referencing a region
  node, and a second finds a verdict over an invariant view's types recorded
  once in the lent branch across a `Fresh` tail loop's hops.
- A value door takes its writer beside a chain paired to the writer's brand,
  whatever brand the chain's tiers sit at; the caching chains are paired to
  the frame's region writer. A door handed a writer and a chain paired to
  another brand does not compile, pinned by the doctest on `Types`.
- Nothing leaving a caching body names a scratch node: its result, each
  argument it passes into a frame and the callee at each birth are walked as
  a knot's copy walks them, every capture, member and barrier handle
  included, and adopted into the region first. A test returns a value retyped
  under a union from a `Shares` caching body and reads it after the scratch
  reset; a second passes a closure, a quote and a member of a cyclic
  component out through non-porous slots and calls, `EVAL`s or crosses each
  after the scratch reset.
- The scratch habitat keeps a free list of released, drained receipt runs; a
  registration takes the first that fits, and the reset clears the list. A
  test runs a `Shares` tail loop that parks once per iteration and holds its
  scratch bytes constant across iterations apart from distinct types.
- The static pass classes every mint site, and a measurement over the example programs counts the branch nodes each sealed
  body would otherwise leave in its host.
- `lattice/src/types` gains one door, an adopt that reads a whole chain rather
  than one branch and its links, and is otherwise unchanged: the routing is a
  choice of chain per site, downstream of the lattice.
- Koan's production build stays free of `unsafe`.
- Every law in [tests/properties.rs](../../lattice/src/types/tests/properties.rs)
  holds unchanged.

**Directions.**

- *Which bodies — decided.* A sealed body that mints, reached by a call or an
  `EVAL`. The program never: its bindings are read across its statements and
  nothing above it is parked. A closed site mints at run only content the same
  at every run ([where each body interns](../../design/type-tiers.md#where-each-body-interns)),
  so the mint count drops it.
- *The site classes — decided.* A `:|` site is classed by its
  [content tree](../../design/modules.md#the-load-and-the-run): closed,
  invariant or varying. A nominal declared per call is classed alike by the type binders
  its declaration reads. Invariance is a greatest fixpoint over the
  tail-call graph the load resolves by name: every parameter starts
  invariant, and stays so only where the argument at its slot on every tail
  edge into its body is a bare invariant parameter of the caller. A body
  whose name escapes as a value keeps every parameter varying; a leaf keyed
  on a binder inside the body is varying; a capture is invariant. A wrong
  class costs growth or warmth, never soundness, which rests on the sealed
  class and the park. Composing trees around a full cycle, which would class
  an argument built over invariant leaves invariant, is
  [its own item](cycle-invariance.md).
- *Which tier — decided.* The warm tier is the scratch branch the frame's
  park holds, released by the scratch habitat's own reset. The asking
  evaluation's parked receipt run holds that reset off until the body's value
  is delivered, so the content lives from the ask to the delivery and no
  count, brand or `unsafe` of its own is needed. The cold tier is the host's
  scratch under `Shares`, which grows with the loop as its bytes do. A
  frame's own receipt is replaced between its statements, so two calls from
  the host's own body in separate statements are cold to each other, while
  two asked at one park, and the statements of a `Shares` callee, are warm.
- *The cold tier under `Fresh` — decided.* The frame's own scratch, held for
  the frame's life by a cellgraph hold and dropped with it, so each hop
  recycles it. The region was rejected: a region node may not reference a lent
  node, so every cold site would first copy its warm children into the region,
  where the reading chain lays in the scratch and reads both sides.
- *`Shares` — decided.* Under `Shares` the warm and cold tiers are both the
  host's scratch, so a `Shares` caching body runs every step on the chain over
  the root, its host's region and that scratch, writing the scratch, and its
  site classes route nothing.
- *Composites — decided.* A composite site is warm where its static type
  reaches invariant carriers alone, cold otherwise, a carrier the load cannot
  name included.
- *Pairing — decided.* A chain carries a pairing brand naming the writer it
  serves, beside the brand its tiers sit at, and a value door takes a writer
  and a chain paired to it. The caching chains are paired to the frame's
  region writer, so every value door keeps its one-brand pairing, no door
  routes per node, and no door's brands relax.
- *The lend — decided.* A cellgraph door lends a host's scratch type branch to
  a `Fresh` frame born under the host's park, at a brand of its own that
  outlives the borrower's scratch and is unrelated to its region, and records
  the cell as naming that scratch until it departs. The lent branch is
  written through a chain over the root and itself alone. The body reads
  through a chain whose own tier is its scratch, with the lent branch as its
  read tier and its region through a link, laid before anything else lays
  that scratch branch. A type it builds stays in that scratch, and one an
  invariant site mints is copied into the lent branch, never referenced
  across. A
  hop hands its predecessor's warm tier to the successor whatever either's
  placement, a `Shares` successor of a `Fresh` predecessor included, so no
  frame's warm tier is a scratch no park holds. Dropping the lend was
  rejected: a `Fresh` loop over an invariant open module would re-lay its
  carrier and signature and recompute every verdict over them each hop.
- *Placement — decided.* Placement stays by return type. A load heuristic
  placing a loop with varying mints `Fresh` was rejected: the `Fresh` bit
  copies predecessor-homed state every hop and the load cannot size that
  state, so a loop accumulating a list would go quadratic to save a small,
  park-bounded growth.
- *Exits — decided.* Before a caching body's value leaves it — its result, an
  argument at a frame birth or tail call, the callee — its types are adopted
  into the region, so no cross-cell door reads a scratch tier. The walk is the
  crossing check's: every part, and every value and handle a knot holds, a
  closure's and a quote's captures, a component's members and a barrier's
  four handles included, since a part keeps its own memo under `Any`, a
  union, a function type or a carrier. Adopt per handle rather than merge the
  branch: the host's scratch is shared by every tenant, so a whole-branch copy
  would carry unrelated transients, and a by-reference merge from scratch into
  region is forbidden by brand. A sealed exit carries no minted content, but
  it may carry a root-only composite the body built in a scratch tier.
- *Ties — decided.* No adopt at a tie. A closure, a quote or a knot member
  leaves a body only through an exit, whose walk reaches its captures and
  members, so a tie adopt would repeat that work at every birth.
- *Settling — decided.* Every adopt out of a caching body reads the whole
  chain the body ran on, through a lattice door that adopts from a chain
  rather than from one branch and its links, so a `Fresh` type whose children
  lie both in the lent branch and in the frame's scratch reaches the region in
  one pass. Adopting through the lent branch was rejected: it keeps the
  type's cold part there until the park ends.
- *Receipt runs — decided.* A free list of released, drained runs in the
  scratch habitat, searched first fit, cleared at the reset. Per-cell reuse
  alone does not bound a tail loop, since every hop and every statement
  evaluation inside it is a new cell born with no run; LIFO alone lets a
  narrow head refuse a wider registration on every hop. A `cellgraph` change.

## Dependencies

The `Shares` tail-hop bytes gap in
[the rewrite project's unplanned work](../rewrite/README.md#unplanned-work)
is compatible only while a recycled successor keeps the branch it was lent.
It also builds on the branch tier
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches))
and the scratch habitat ([the cell](../../cellgraph/README.md#the-cell)).

**Requires:** none.

**Unblocks:**

- [Invariance around a tail cycle](cycle-invariance.md) — refines the site
  classes.

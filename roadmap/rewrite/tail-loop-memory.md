# Flat memory for a tail loop

**Problem.** A tail recursion holds a constant number of cells
([tails](../../src/program/README.md#frames-contracts-and-tails)), but the
region of the cell it was called from grows with every hop until the call
returns, whichever placement the loop takes. Over 64 and 256 hops of a keyed
`PASS m link` that calls a `Fresh` helper each hop, every live region (absorbed
bumps included) gains about 880 B per hop when `PASS` returns `Str` (`Shares`),
1400 B when it also binds a view, and 2000 B when it returns `Number` (`Fresh`).
`RUNS` in the [tail tests](../../src/dispatch/tests/tail.rs) grows like the
`Fresh` case. A loop whose hops stay under their frame's settlement
(`tail_under`, [`evaluate.rs`](../../src/dispatch/evaluate.rs)) and one that
seeks a settlement each hop (`tail_seeking`) grow by the same bytes. The bytes
come in three ways:

- A frame's last statement runs as an evaluation that is a tenant of the
  frame's *parent* ([`run`](../../src/program/body.rs)) and views the frame's
  activation. A view always pins ([`KBundle::weight`](../../src/program/bundle.rs)),
  so a `Fresh` frame is pledged to splice into that parent. At its death, its
  whole bump, garbage included, goes into the parent's bundle instead of back
  to the spare list. Every `Fresh` call whose body tails does the same to its
  caller: in a loop, every hop's frame and every `Fresh` callee a hop makes.
- A `Shares` frame and its evaluations are tenants of the host. Its activation,
  its temporaries and what it binds are written straight into the host's
  region, and so are the types a porous body builds, into the host's
  [branch](../../lattice/src/types/identity.md#storage-a-root-and-its-branches).
  The hand-off evaluation writes the next hop's argument record there under
  either placement.
- The hand-off evaluation is a tenant, so its scratch is the host's, which
  cannot reset while the host is parked on its receipt run. The caching chain
  and parked scratch state grow there per hop.

Two facts of the substrate bound every fix. A hop's successor is born a sibling
of its predecessor; siblings are `Apart`, so whatever crosses between them is a
forced copy that never consults the verdict, and the copy is total — a value's
parts are plain pointers, so the copy cannot tell a cons cell built this hop
from the rest of a chain homed in the root
([crossing](../../src/values/README.md#crossing)). And a carrier's home is the
cell that lifted the value, not where the value's bytes are
([`alloc_into`](../../cellgraph/src/graph.rs)): a part of a parameter, lifted in
a frame, is homed in the frame. So a crossing today can only pin, which keeps
the predecessor's whole region, or rebuild the value whole, parts homed above
included — `link` would copy the rest of its chain every hop.

**Acceptance criteria.**

- A test sums the bytes every live region holds, absorbed bumps included, at
  each tail of a loop at 16, 64 and 256 hops, and finds them flat in the depth
  for two loops: a chain walk (`PASS m link`) and a loop that rebuilds a record
  whole each hop. Each runs both hopping under its frame's settlement
  (`tail_under`) and seeking one each hop (`tail_seeking`).
- A chain walk copies no part of the chain per hop: the bytes a hop's birth
  copies are a constant independent of the chain's length.
- A loop that conses one element onto a list each hop copies a constant number
  of bytes per hop, independent of the list's length.
- A loop that conses onto a list holds, across the loop, at most a constant
  times the weight of the values its latest birth threads, plus one hop's
  region.
- The host's region, its scratch and its decision memo hold the same bytes
  after 256 hops as after 16.
- A hop still keeps its frame's settlement and level: the tail tests' count of
  hops under a frame's own settlement is unchanged.
- A hop has one placement: `Step::tail` takes no `Placement`, and `placement_of`
  in [`body.rs`](../../src/program/body.rs) is gone.
- No `unsafe` block is added to koan. One added to cellgraph sits behind a safe
  door whose safety contract is stated beside it.
- [Recursion: the iteration idiom](../../tutorial/06-pattern-matching.md#recursion-the-iteration-idiom)
  states what a loop keeps while it runs — the values its tail call hands on
  and what they reach — and that everything else a hop builds is dropped at the
  next hop.

**Directions.**

- *Where threaded state lives — decided.* In a **loop cell**: a tree cell under
  the host, with no continuation, that is the *parent* of every hop
  ([tree cells](../../cellgraph/src/tree/README.md)). Every value the loop
  threads — a binder in the body's threaded set or an inline tail argument — is
  built into it through `alloc_into`, the one door that builds into another
  cell's region and hands back a value homed there. What it holds pins into
  each hop free, by ancestry; it lends no region, since a lend would read as
  `Shares` to the hop's `placement()` ([`action.rs`](../../src/scheduler/action.rs))
  and buys nothing ancestry does not. The hop's own region — activation,
  temporaries, frame-local binders, callee splices, its argument record — is
  reclaimed every hop. The loop cell is released when the loop delivers; its
  bytes reach the host by the ordinary pledge-and-splice, or a copy if the
  result is small. A loop whose bodies thread nothing (a walker, a scalar loop)
  gets no loop cell; its hops stay siblings under the host. New in the drain:
  a loop-cell field on `Provenance`, which is brand-free and already crosses a
  hop verbatim; a create with no continuation; a release at delivery that
  releases no arena borrow.
- *A hop never pledges — decided.* Koan supplies the crossing verdict
  ([`crossing.rs`](../../src/values/crossing.rs)), and an upward `alloc_into`
  consults it, priced by the source's splice price. At the loop cell's door the
  verdict is `Copy` for any operand homed in a hop, whatever the price; an
  operand homed in the loop cell or the root pins free. So the only bytes that
  leave a hop are the ones it threads, by a copy that is O(what this hop built
  and threads).
- *The hand-off evaluation — decided.* A *child* of the frame, so its view of
  the activation pins into a descendant at no price and no pledge, that
  **inherits the frame's provenance**, so its successor is born under the loop
  cell (or the host), never under the dead frame. The frame is released after
  the evaluation's first step and disposes when the evaluation does; the
  evaluation disposes after the successor's first step. No cell in the loop is
  a tenant of the host, which closes the scratch path. `Provenance` gains a
  third role: `parent` is today both the cell that receives the result and the
  birth parent ([`drain.rs`](../../src/scheduler/drain.rs)); in a loop the host
  receives, the loop cell births the successor, and the evaluation's own birth
  parent is the frame.
- *The successor's birth is built where it lives, one operand per argument —
  decided.* The successor's `KBirth::Call` is built in the successor's own
  region at its creation, from one operand per argument rather than one for the
  record: a scalar is inline; an argument the frame **received** — a parameter,
  or a part of one read through the builtin `ATTR` alone, as
  [`received`](../../src/dispatch/walk.rs) already decides for the type tiers —
  reuses the frame's own birth carrier, one home for the whole record; a
  threaded build carries the `Ready` its `alloc_into` returned. Every such
  operand is homed in an ancestor of the successor and pins free, and a pinned
  crossing links the successor's type branch to its home's
  ([`cross`](../../src/program/bundle.rs)). Anything else is a forced copy, the
  price of a static miss. The record struct, `contributed`, `handed` and the
  level die with the hop. The successor builds it in its first step, through
  the existing `redeem`, `alloc_into` and `alloc_here`, while its predecessor
  is still live; no create door. A copy that keeps parts by reference is ruled
  out: a copied operand lands at an unrelated `'severed` brand, so keeping a
  part is a brand cast, sound only by a run-time range check.
- *Kept carriers — decided.* A frame keeps, in a slot table in its region, a
  carrier at rest at the true home of each argument, of its callee and of each
  binder its evaluation delivered. A spawned frame projects its arguments' and
  callee's out of its birth carrier, through a new cellgraph door that projects
  a carrier onto a part of its value at the same reach and home; a hop's
  successor keeps those its first step pinned or threaded.
- *Held values — decided.* A value an evaluation holds carries its kept carrier
  where it has one: a name read of a kept slot, a builtin `ATTR` field read of a
  held value (by label path), and a child's result. A finish hands a kept value
  to a home at or under its carrier's home unchanged, through a new cellgraph
  door, and re-lays a restamp there over it. A construction — a list, dict or
  record literal, a newtype construction — builds in its home over its parts,
  which it asks for with `Forwards`. So a threaded construction builds in the
  loop cell with its received parts pinned, and a call that returns its
  parameter, or a field of one, places nothing new in its caller.
- *The threaded set — decided.* Static, per body, sealed on `BodyShape` beside
  `last_reads` ([`shape.rs`](../../src/scope/shape.rs)): the binders in the
  backward closure over the builder's read edges (`Draft::edges`,
  [`build.rs`](../../src/scope/shape/build.rs)) from the last statement's reads,
  plus the last statement's inline arguments, with type slots filtered out —
  `last_reads` lists every type slot whole, which would otherwise give every
  body with a type binder a loop cell — and less what the frame received, judged
  syntactically as `received` judges it. Each member is asked with a use whose
  home is the loop cell, where the frame's provenance carries one. It composes without knowing the loop's
  component, which matters because the runtime tails more than the static
  component ([tails](../../src/scope/shapes/tails.rs)): any last non-binding
  statement whose contract is kept, through function-typed parameters too. A
  miss under-approximating costs a forced copy at the hop, never safety;
  over-approximating keeps that binder in the loop cell until compaction.
- *Compaction — decided.* The loop cell memoizes the weight of the values the
  latest birth threads. When its bytes exceed twice that weight, the next hop
  builds its birth into a fresh loop cell, copying the live threaded values
  once, and the old cell is released. Amortized O(1) per hop, confined to the
  loop cell; a loop that rebuilds a record whole each hop keeps at most one
  dead generation.
- *When the loop cell is created — decided.* At the first hop that hands on a
  non-scalar value of its frame's threaded set when its provenance carries no
  loop cell, so a walker and a scalar loop never get one. The first frame's
  threaded values are copied into it once, at loop entry. The drain reads no
  shapes, so the hop says it threads, with the weight it threads.
- *The decision memo's lay — decided.* `memo_home` is the host and `memo_lay`
  writes it through `alloc_into` when it lays an entry
  ([`action.rs`](../../src/scheduler/action.rs)), within a per-region lay
  budget, so the host's memo is bounded. The host criterion's test verifies a
  loop lays nothing per hop.
- *Precision of kept carriers — open.* A capture, a top-level name or a module
  member passed on a tail call has no kept carrier, so it is copied into the
  loop cell each hop it is passed. A spawned call's kept carriers are projected
  from its birth, homed where its caller built the argument record, so a helper
  that returns a part of its argument inside a threaded value copies that part.
  Candidates: kept carriers for captures through the callee's carrier; births
  built from one operand per argument for every call. Recommended: ship
  without, and add either as a profile measurement asks for it.
- *Faults — deferred.* Under [faults](../conditionals/faults.md) a located
  fault lives in the faulting cell's region; a hand-off evaluation that is a
  tree cell would have to be kept, and the loop cell with it. Decided when
  faults land.
- *Slices — deferred.* When [slicing](../metaprogramming/slicing-and-splicing.md)
  lands as views, `rest xs` is a view of root bytes lifted in the hop; threaded,
  the view struct is copied and its target pins free. Confirmed when that item
  lands.

## Dependencies

[Faults](../conditionals/faults.md) and
[slicing](../metaprogramming/slicing-and-splicing.md) each touch the hand-off;
neither is a prerequisite.

**Requires:** none.

**Unblocks:** none.

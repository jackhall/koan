# Knot layout

A `memory` shape for a group of values that refer to each other: one run of
nodes whose sibling references are indices into the run rather than pointers.

**Problem.** A value cannot refer to itself or join a reference cycle. Every
composite is born through a door that takes the region's `Writer` from finished
parts ([values](../../src/values/README.md#what-a-value-is)), so a field cannot
point at a value that does not exist yet. The type side has an answer: a group
of mutually recursive nominals is interned at once in run storage, each member
a `(SCC digest, index)` handle, so a sibling edge is an index and never a
pointer ([type lattice](../../src/type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)).
Region storage has no counterpart, and `memory`'s one cell-tier shape is the
[slot array](../../src/memory/README.md#the-slot-array), a table of write-once
bindings. Both circular data and a group of mutually recursive functions need
the same thing: members born together, with edges that name a sibling that may
not be written yet. Mutually recursive functions cannot be built one after the
other, because a closure's bindings are a shallow copy taken once no binding it
copies is pending
([scope](scope-on-values-and-types.md)), so a group of them is one knot or
nothing. [Constructing circular values](../old_foundation/circular-value-construction.md)
records the requirement from the old runtime's side; this item resolves its
open representation question in favour of the index-edged group.

**Acceptance criteria.**

- `cellgraph`'s `Writer` ships a third settled-width verb, `thin_run`: one
  allocation holding a length header with the run of *N* elements laid down
  after it at their own alignment, handed back as `ThinRun`, a `Copy` handle
  one pointer wide, generic over its element, under the same `const` drop-glue
  assert as `fill`. Its `unsafe` is the crate's, pinned by cellgraph's Miri
  slate, and koan's `src/` carries no `unsafe`.
- `memory` ships the knot over it: `Knot`, a `Copy` handle one pointer wide,
  generic over its payload, whose nodes rest in a cell's region and whose
  construction restates the drop-glue assert so a payload carrying drop glue
  fails the build naming the knot.
- An edge is a `u32` node-index newtype with no public constructor, minted
  only by a `KnotPlan` sized by the knot's count, so every minted edge is below
  that count; the plan is neither `Copy` nor `Clone` and the tie consumes it,
  so one plan ties exactly one knot.
- A node is read as a `(knot, index)` member, and an edge is resolved only
  through a member or its knot, never against a bare run; resolving an edge
  against a knot whose count it does not fit panics. No node holds a pointer
  to a sibling, so a copy of the run carries every edge verbatim and a copier
  never follows one.
- Construction takes finished payloads only, with the count fixed before the
  first payload: a node whose edge names a sibling not yet written is staged in
  the consumer's scratch, without a placeholder and without interior
  mutability, and copied out by the tie. A knot never grows.
- `memory`'s import rule is unchanged, and
  [`src/memory/README.md`](../../src/memory/README.md) describes the shape as
  shipped.

**Directions.**

- *Where the unsafe lives — decided.* In `cellgraph`, as one `Writer` verb.
  A thin handle over a header-then-run allocation cannot be built in safe Rust,
  and it buys one allocation instead of two, 16 fewer bytes per knot, and a
  resolve whose element address is arithmetic off the handle rather than a load
  through a header. `cellgraph` already owns every `unsafe` in the workspace and
  the slate that pins it, so the verb goes there and the knot stays safe code.
- *Edge confinement — decided.* An edge is a plain index newtype, not an index
  branded to its knot. A brand that told two knots apart would be a fresh
  invariant lifetime per knot, quantified over its construction closure, and a
  knot rests at `'cell` and is captured by continuations across steps, where
  that lifetime cannot follow; every knot in one cell shares `'cell`, which
  distinguishes nothing. Branding the construction window instead would need
  a lifetime family per payload type and a retype at the tie, and would still
  leave an edge read out of a finished node unbranded. The wrong-knot case is
  closed as far as it can be by the plan: consumed at the tie, so a count is
  never shared, and a count mismatch panics at resolve. The residual case, an
  edge from one plan inside a payload tied by another whose count happens to
  fit, is a consumer bug of the same class as indexing one `Vec` with another's
  index, and is documented on the minting verb.
- *Graphs built across steps — decided.* A knot is tied once. A node of a
  later knot may hold a member of a finished knot as an ordinary
  pointer-carrying payload field, which the knot layer neither mints nor sees;
  knots therefore form a DAG, cycles live only inside a knot, and a finished
  knot never gains an edge to a newer node. This is what write-once values
  already require, so no growth verb is shipped.
- *Copying and weight — decided.* The shape ships no crossing verb and no
  weight: a copy is the consumer's, rebuilt through the destination's writer
  payload by payload with the edges carried as they are, and its price is
  whatever the payloads weigh. `memory` names neither.
- *Consumers — open, out of scope.* Circular data values — a knot-member arm on
  `Value`, equality by bisimulation over `(knot, index)` pairs so that two knots
  of equivalent topology compare equal wherever their indices fall, and a
  renderer that terminates on a cycle — are the `values` layer's. Groups of
  mutually recursive functions as knots of function values are the scope and
  callable-values layers', and a function group's *shape* — which sibling
  definitions form a knot, computed as the type side computes a component — may
  rest in program storage while its values rest in a cell's region. Neither
  consumer is decided here.
- *The `Value` word — decided.* `Value` is 24 bytes and asserted so
  ([values.rs](../../src/values.rs)); a member arm carries the thin knot pointer
  and a `u32` index, 16 bytes beside the tag, which is why the knot is reached
  through a header and never a fat slice.
- *The type lattice on the knot — open.* A sealed recursive group is a run of
  members with index edges, the shape this item ships; the lattice could be
  ported onto it for recursive types instead of keeping a twin. Recommended:
  decide once the value-side consumer stands, so the shape is shaped by two
  users rather than one.

## Dependencies

**Requires:** none — [memory](../../src/memory/README.md) ships.

**Unblocks:**

- [Callable values](callable-values.md) — a definition window's mutually recursive functions are one knot.
- [Constructing circular values](../old_foundation/circular-value-construction.md) — carried edge; the index-edged group is its representation, and its leak criterion is moot under region release.

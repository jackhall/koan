# Type tiers

A type the load builds lives in program storage for the life of the program. A
type the run builds lives in one of three shorter tiers, chosen by what can
still reach it, and is released with that tier: a region's branch with the
region, a scratch branch with the step, a sealed body's shape branch when its
last live activation returns.

This doc tells that story across the modules that carry it.
[The type lattice](../lattice/src/types/identity.md#storage-a-root-and-its-branches)
holds each tier's table and the node layout, [the static pass](../src/dispatch/README.md#static-types)
types every site and classifies every body, [the scheduler](../src/scheduler/README.md)
counts activations at frame transitions, [values](../src/values/README.md#crossing)
re-intern at a crossing and [knots](../src/knot/README.md#a-lambda) at a
closure's birth, and [cellgraph](../cellgraph/README.md#the-contract-four-embedder-types)
brands each tier's door.

## Four tiers

| tier | lives from | to | holds |
|---|---|---|---|
| root | the load | the program's end | every type the load builds |
| region branch | the region's birth | its death | a porous body's run-built types, and foreign content |
| scratch branch | a step's start | its end | the step's scratch types |
| shape branch | a sealed `Shares` body's outermost entry | its return | that body's self-contained content |

A handle is its content's digest, so it names one type in every tier, and a
reset moves where a type resolves, never what it is. Every tier's table is a
[`DigestTable`](../lattice/src/bump/digest_table.rs) differing only in what
lays its runs. A lookup probes the tiers a step reads, shortest-lived first.
The root is closed after the load, so a site whose static type is non-porous
holds a root handle and skips every branch.

## A node lives in a tier its children outlive

A node names its children by reference, so a node's tier is a lifetime
question per node: it lives in a tier that every child's tier outlives.
Scratch is under everything; region and root are ordered; shape and root are
ordered; region and shape are incomparable, since under recursion an inner
host dies before the outer activation returns, and a host above the outer
activation outlives it. So nothing references across the line between region
and shape in either direction. Content crosses that line by copy: a node
relaid in the destination tier, its children relaid before it,
insert-if-absent, so a content relaid twice is one node.

## Porosity

A reified atom — a carrier [the view door](../src/knot/module/README.md#the-view-door)
mints, a nominal born per call — lies under almost nothing: itself, a union
holding it, and `Any` ([the order](../lattice/src/types/laws.md#the-order-is-a-partial-order)).
A value carrying one can pass a slot only where the slot's static type has a
**porous** position: a lexical or quantified variable, or an unpinned head
parameter of a declared signature, anywhere; `Any` at a covariant position;
`Never` at a contravariant position, since the order flips at a function
type's parameters and a function over a carrier parameter lies under
`FN :{x :Never} -> Number`. An open capture, typed `[Never, Any]`, is porous by
definition. Porosity is a property of a root type, folded once at the load as
a flag beside the lattice's others: two bits per node, swapped at a function
type's parameter position, the covariant one read at the root.

Slot porosity is the lookup side only. A closure typed `Number -> Number` can
hold a captured module whose memo is a branch handle; the type does not show
it, and a reader at that slot resolves a root memo. What the capture does is
bound a lifetime, below.

## Three classes of body

Every body is classified where the program loads. A reified-dependent value
enters a body only through a porous parameter or capture, a surfaced
`USING … SCOPE` name typed rigid or unknown, an `EVAL`'s result, a hole, or
the result of a call whose static type is porous; it leaves only through a
porous result or a porous argument slot of a call the body makes.

- **inert**: no porous entry and no minting door. It never sees a branch
  handle and probes the root alone.
- **sealed**: it mints, but has no porous exit slot. Nothing it minted is
  reachable after it returns.
- **porous**: it has a porous exit slot. The static pass lists the sites where
  a type can cross, and only those probe a branch.

A closure is the one way a type leaves under a non-porous slot, through a
capture, and that route is closed at the closure's birth rather than by
classification: a closure born over a value whose memo resolves in a shape
branch has that branch merged into the region branch of the habitat the
closure is laid in, so it never holds a shape handle when it leaves, and the
crossing rules carry its captured types from there. The static pass marks the
birth sites that can pay, those with a capture typed over an atom the body
births; every other birth costs nothing.

## Where each body interns

A porous body interns into the branch of the habitat its writer lays into —
its region, or its host's for a `Shares` tenant — through the header door
beside the writer, so interning into another habitat's branch does not
compile. A sealed body's content splits by whose handles its children are:

- **self-contained**: atoms it births and composites over them and root types
  — a view's carriers and the signature's templates at them, a per-call
  nominal, literals over those. Into its shape branch.
- **foreign**: anything over a handle the caller passed. The frame door builds
  these on every activation of a quantified callee, retyping each parameter
  and the return with the call's solution substituted
  ([frames](../src/program/README.md#frames-contracts-and-tails)), and memos
  of values built over argument parts do the same. Into the region branch,
  with any shape-branch child relaid there. A mixed node — a record of a
  minted carrier and an argument — has no tier both outlive, which is why the
  region wins.

The static pass decides the split per template at the load. A value binder is
atom-born where its right-hand side is a view site in this body or a read of
such a binder; a template is self-contained when its run-bound positions are
projections off atom-born locals and surfaced names whose `USING` operand is
atom-born, and foreign where it holds a `FOR ALL` name, a `:Type` parameter,
a hole, a mark, or a projection off a parameter, capture or call result. The
pass is exact where it says self-contained and says foreign wherever unsure:
self-contained content in the region costs a relay and the early release,
while foreign content in the shape tier would dangle on a returned value, and
the shape writer resolves through the shape branch and the root alone, so a
foreign child misses at the lay. Only a body with no porous entry mints
self-contained content alone; every quantified callee has porous entry and
mints foreign content too.

A sealed body placed `Fresh` lays no shape branch and interns into its region
as a porous body does. Its region is recycled per activation and per tail
hop, and a hop's forced copy relays what the state carries, so a loop
ascribing new content each hop holds a constant node count; a shape branch
could not reset at a hop, since the successor's first step interns before the
predecessor's release. What `Fresh` gives up is warmth across activations, a
handful of probes against a lay. The shape tier serves the sealed `Shares`
body, whose types would otherwise linger in its host until the host finishes.

## The shape branch

**Its brand** is the scratch habitat's proof at a different cadence. The
branch rests in a record beside the body shape in program storage, which the
shape references and which holds the count and the presizing memo too. The
slot is erased between steps, reattached at a brand quantified per step and
invariant, and cleared at the reset. Three facts discharge the reattach, each cited by its SAFETY
comment — the one `unsafe` in koan outside the test build:

1. the reset runs only in the drain, between steps, where no step brand is
   live;
2. the slot is cleared at the reset, so nothing stale is reattached;
3. the relations are `'graph: 'shape: 'scratch` and none with `'here` in
   either direction. `'here: 'shape` would let a shape node hold a region node
   past the region's death, and a later epoch's lookup of equal content read
   it; the reverse would let a region node outlive the reset. Each direction
   is pinned by a compile-fail doctest.

**Its count** is the scheduler's: up at every frame entry, a hop's successor
included, down at every release, a hop's predecessor included, and the branch
resets when the count reaches zero at a release. It counts activations,
parked included, since a parked scratch state may hold scratch nodes over
shape nodes. The [hand-off order](../src/scheduler/README.md#tail-hops) — the
successor runs its first step before the predecessor is released — makes a
same-shape loop go one, two, one per hop and reach zero only at its final
return, and makes a hop between shapes reset the predecessor's branch after
the successor's first step ran the crossing. That reset is sound because what
crossed went to the successor's region tier: a `Fresh` hop's forced copy
relays each memo, and a `Shares` hop's pin merges the predecessor's shape
branch into the host's region branch, as a pin merges a home's branch.

**Its growth** is a high-water mark, never a leak: every nested activation's
distinct content accumulates while the count is positive, and the reset
reclaims it whole, stranded runs included. A shape branch is laid at its
smallest size and grows by laying runs, as a region branch does.

## Verdicts

A verdict is never load-bearing, so its placement is retention alone: it is
recorded in the table of the shortest-lived tier holding one of its two
types, and released with that tier.

## Open work

- [Body classes](../roadmap/type-tiers/body-classes.md) — the porosity flag and
  the class fold at the load.
- [Type branches](../roadmap/type-tiers/type-branches.md) — the region and
  scratch tiers, the chain, and the crossing's re-intern.
- [Shape branches](../roadmap/type-tiers/shape-branches.md) — the shape tier,
  its brand and count, and the merge at a closure's birth.
- [Presizing a branch](../roadmap/type-tiers/branch-presizing.md) — a branch
  laid at a predicted size, the shape branch's from the memo in its record.

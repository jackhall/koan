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
reset moves where a type resolves, never what it is. Every branch's table is a
[`DigestTable`](../lattice/src/bump/digest_table.rs) differing only in what
lays its runs; the root's is a hashbrown table the load grows and freezes. A
node lookup probes the tiers a step reads longest-lived first, the root before
any branch, and a read by handle then follows a branch's link to its
spawner's, caching the hit; a verdict lookup probes them shortest-lived first.
The root is closed after the load, so every type the run builds lives in a
branch, a composite over root types alone included.

A crossing carries a value's types to where it lands
([storage](../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
A pinned birth into a cell that has laid no branch — a spawned child, a hop's
successor in a new cell — links the cell's branch to its home's, so a birth
costs the same whatever the home holds. Every other pin — a result, a birth
into a tenant, whose branch is its host's — merges the home's branch, and what
the home reaches through its links up to a branch the destination reaches,
into the destination's; a copy re-lays each type it reads. No pin walks the
value it carries.

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

A type minted at run — a carrier [the view door](../src/knot/module/README.md#the-view-door)
mints, a nominal declared per call — and every type built over one is
**minted content**. Porosity says where minted content can sit, as two flags
folded once per type at the load, beside the lattice's others.

A type is **porous** where minted content can lie under it or fit it. A
carrier is an atom of [the order](../lattice/src/types/laws.md#the-order-is-a-partial-order),
under itself, a union holding it and `Any` alone, but a type built over one
reaches further: a list or record over it lies under `Value`, a record with an
extra field at it lies under every record type by width, and a view's
signature lies under `Module`, the top of the signature types, and fits every
signature its members satisfy. At a contravariant position every type but
`Any` admits `P | C`, so `FN :{x :(Number | C)} -> Null` lies under
`FN :{x :Number} -> Null`. So `Any`, `Value`, every record type, every
signature type and every variable are porous; a function or shape type is
porous where a parameter or slot is not typed `Any` or where its return is;
and a list, dict, union or family application is porous where a child is.

A type is **exposing** where a value seen at it can hand a reader a part or
payload of minted content. A part keeps its own memo under `Any`, a union, a
function type or a carrier ([values](../src/values/README.md#the-type-memo-and-satisfies)),
and `Value` or a variable restamps nothing; a module reports its own memo
under any signature; a type value's payload is the type it names, whatever
its kind; a nominal's payload is read at its representation. So `Any`,
`Value`, every kind, every signature type, every variable, a carrier and a
nominal are exposing; a union is exposing where it is porous or a child is
exposing; and a list, dict, record or family application is exposing where a
child is exposing or is a porous function type. A function type exposes
nothing: what a function reaches it reaches through its captures, which the
tie rule below closes.

A value is **porous** where its static type's upper end is porous or
exposing, and, where that type is exact and closed, where it is exposing: an
exact closed site carries that very type, and the surface door hides what the
type does not name, so a retyped record shows no extra field to any reader
but a knot's copy, which the tie rule closes too. An open capture, typed
`[Never, Any]`, is porous by definition.

Porosity promises only that a value at a non-porous site reaches no minted
content, never that its handles are the root's (§ Four tiers).

## Two classes of body

Every frame body — the program, a callable, a quote's code — is classified
where the program loads, the arms and module bodies it runs counted with it. A
value enters a body through a parameter, a capture, a surfaced
`USING … SCOPE` name, an `EVAL`'s result, a hole or a call's result, and
minted content enters only where that value is porous. It leaves only through
the body's return or an argument of a call the body makes into a frame — a
registration, a function called by name, a surfaced head, a candidate the load
cannot read — and only where a porous value meets a porous slot there; and
through every `EVAL`, which offers values to the code's open `\` marks at
slots the body does not type. A builtin's native runs in the caller's step and
enters no frame. A body mints at a `:|` ascription, which hides each unpinned
head parameter behind a carrier, and at a nominal declaration the load leaves
unknown, declared each time its unit runs; `:!` mints nothing, since it
carries each head parameter at what *fits* solved.

- **sealed**: no porous exit. Nothing it minted is reachable after it
  returns.
- **porous**: a porous exit.

Beside a sealed body the load fixes whether it lays a shape branch: it mints,
and it is placed `Shares` — a callable by its declared return, a quote's code
always, the program never.

A tie — a closure's or a quote's birth, a cyclic component's — is the one way
minted content leaves under a non-porous slot: through a closure's or a
quote's captures, since neither a function type nor a code type exposes what
its value captures, and through the cells a knot member's seen type hides,
since a knot is [copied](../src/values/README.md#crossing) at each node's own
memo. That route is closed at the tie rather than by classification: a knot
tied over a value whose memo resolves in a shape branch has that branch merged
into the region branch of the habitat it is laid in, so no member holds a
shape handle when it leaves, and the crossing rules carry its types from
there. The static pass marks the ties that can pay, those with a capture or a
member typed over an atom the body births; every other tie costs nothing.

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
branch into the host's region branch.

**Its growth** is a high-water mark, never a leak: every nested activation's
distinct content accumulates while the count is positive, and the reset
reclaims it whole, stranded runs included. A shape branch is laid at its
smallest size and grows by laying runs, as a region branch does.

## Verdicts

A verdict is never load-bearing, so its placement is retention alone: it is
recorded in the table of the shortest-lived tier holding one of its two
types, and released with that tier.

## Open work

- [A merge's reach check](../roadmap/type-tiers/merge-reach-check.md) — a
  merge whose cost does not grow with its destination's link depth.
- [Shape branches](../roadmap/type-tiers/shape-branches.md) — the shape tier,
  its brand and count, and the merge at a closure's birth.
- [Presizing a branch](../roadmap/type-tiers/branch-presizing.md) — a branch
  laid at a predicted size, the shape branch's from the memo in its record.

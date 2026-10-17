# build

The build tree: the cells the load and its resumptions run in, beside the run
tree and under the same drain.

A koan program is two trees of [cellgraph](../../cellgraph/README.md) cells.
The **run tree** is the call tree: the program's run root with every frame a
tree cell or a tenant under it. The **build tree** is where the program's
text is decided: the build root's step writes the written program's shapes,
its type facts and the root type tier, and a specialization holds what a
binding settles, the verdicts over it and the types it mints. Its cells are
**build cells**; the run tree's are **run cells**. The
[scheduler](../scheduler/README.md) drains both, so type solving interleaves
with value computation as steps, and a run cell reads what a binding settled
through one counted borrow.

This module owns the build tree and the solver's driver: the build cells, the
type facts with their edges, the frontiers and seams that decide where a
resumption lands, the specialization tables and what they cost, and the
borrow a run cell takes. Its neighbours keep what they own:
[elaborate](../elaborate/README.md) the per-expression typing rules and the
type channel's vocabulary, called from a build cell's step;
[scope](../scope/README.md) the shapes and name resolution, built by the build
root's step and stored in program storage; [dispatch](../dispatch/README.md)
selection, reading verdicts through the borrow;
[memory](../memory/README.md) program storage, which holds the parse, the
builtin table and what the build root's step writes for the written program;
the scheduler the drain, which gains build cells as enterable work, the load
door and the lookup before a birth, and nothing else.

## Two trees under one drain

- **The run tree** is unchanged by the build tree's presence. Liveness is
  structural, holds point up the chain, results splice upward, crossings
  price as the [tree pool](../../cellgraph/src/tree/README.md) states.
- **The build root** is a slab cell beside the run root, the two slab slots
  the graph holds, and the build tree's one **core** cell: it lives as long as
  the program. Its step writes the written program's shapes, every type fact,
  the value channel, the caching records, the solver's fixed facts and its
  residual, which rest in program storage, and the **root type tier**, the
  frozen root in the owner's registry, read by everything after and written
  by nothing else. Its region holds the root-level specialization table.
- **A specialization** is a tree cell under the build root or under another
  specialization: one **seam** under one observed **binding** (below). It
  holds every type fact the binding settled, in every body under the seam,
  the verdicts those settlements produced, and a type branch holding what the
  settling interned. Nesting follows binding order along the lexical chain: a
  binding of callable `inner`'s rigids, taken in a frame running under the
  specialization of its encloser `outer` at `Elt := Number`, is a child of
  that specialization, so a specialization's ancestors are every enclosing
  binding's. A specialization is cache: its parent can evict it, and a miss
  rebuilds it.
- **A composed-code cell** is born with a code value composed at run time. It
  is a chain root in the one-chain sense of
  [quotes](../scope/README.md#quotes), built from the code alone, but a tree
  cell under the build root, since it reads the root's types. The code
  value's home run cell counts it, and it is released when that count reaches
  zero.
- **One core, not one per segment.** Every written segment is program-long, a
  specialization's key already names its segment by its index, and the root
  type tier wants one table. Written code has
  one core; composed code adds a build cell per code value.
- A specialization never rebuilds shapes: written code's shapes live in
  program storage, composed code's in its cell. The raw parse stays in program
  storage.

**Lingo.** The solver's unit is a **type fact**, the shape's
[load-time type](../scope/README.md#load-time-types) carrying its edges: data
the build writes, in program storage for the written program and in a
specialization's region for what a binding settles, never a cell of the
graph. "Build cell" and "run cell" are the graph's, and a "node" in this doc
is the type lattice's. An unqualified "cell" is avoided.

## Type facts and the solver

A type fact is a static type as an interval, or a verdict, and the typing
rules are monotone: law 6 of the [lattice](../../lattice/src/types/laws.md)
makes every rule map both ends, so a fact only narrows and a verdict moves
only from *maybe* to *always* or *never*. A fact is asserted only where it
holds under every refinement of the unknowns it reads.

The driver is a **worklist over the facts**, in the manner of Kildall. A rule
that reads an unknown **parks** its fact on that unknown and records the
**edge**. The load runs the worklist to **quiescence**. Its output is the fixed
facts and the **residual**: the parked facts and their edges. A **binding**
unparks the facts on its edge; their rules re-run, can only narrow, and what
they produce goes to the specialization the binding names. The unknowns a
binding removes are a parameter's content, which is its path class
([paths](../dispatch/README.md#paths)), a `FOR ALL` group's solution and a
`:Type` argument at frame entry; a lexical variable's type at the statement
that binds it; and a path variable's hop.

An unknown is named by its **lexical address**, the segment and the
coordinate the shape binds it at, never by its lattice node, which every body
numbering its chain alike shares. A mention of a fellow member of its own
component, itself included, is no unknown: it reads the fellow's annotation,
the type its birth declares or `[Never, Any]` and records no edge, so the
facts form no cycle.

The lattice is untouched: `substitute`, *fits*, `judge_by_class` and the
unifier are what the rules call, and this module lives downstream of them.

## Frontiers and seams

- **A frontier.** At quiescence of the load, every parked fact waits on a
  definite set of unknowns after transitive propagation. The residual's edges
  are exactly that set; the driver computes it and nothing more is needed.
- **A seam.** A binding site at which some frontier's last member binds:
  frame entry when a rigid's binding completes a frontier, a statement when a
  binding there completes one. A statement that binds nothing any frontier
  waits on is no seam, and costs its run cell nothing. The load lists the
  seams per segment. An **entry seam** keys a specialization
  ([below](#keys-which-bindings-make-a-specialization)). A **statement
  seam** is cold: its binding is content the run produces, an `EVAL`'s or an
  action's result, which keys nothing, so the run cell resumes the frontier
  into its own scratch and its lend never changes. A statement's binding that
  entry content determines — a `MATCH` arm's `it` over a parameter, a
  `USING … SCOPE` key over one, a path hop rooted at one — is settled at the
  entry seam, since a load with that content declared would type it closed.
- **One build cell per seam per observed binding.** Facts that wake together,
  settle together and mint types under the same binding are one build cell
  with one type branch, never several. The build cell holds every fact any
  frontier completes at that seam, in every body under it: two bodies nested
  in `outer` that both wait on `Elt` are solved in `outer`'s specialization
  at `Elt := Number`. Frames coupled by the same rigids share a build cell,
  and one binding settles a whole section of code, the frontier class rather
  than the frame.
- **Resumption scope is per frontier.** A frontier whose members all bind at
  entry resumes at entry; one with a mid-body member resumes at that
  statement. The scope is read off where the frontier's members bind, never
  set program-wide.
- **The tree is a chain.** Binding order follows the lexical chain: entry
  before statements, statements in [unit order](../scope/README.md#units), an
  outer body's seams before an inner body's. A specialization's parent is the
  specialization of the seam preceding it in that order, so every run cell's
  type chain is linear. The chain is rarely probed at run time: a settled
  fact is content, and only a verdict left *maybe* reads the chain.
- **Code finishes building before it runs.** A body's call never precedes
  the binding of a variable it captures: the shape orders each unit after
  every unit that binds a slot it reads, a deferred mention waits on its
  binder, and a component with an eager mention of its own member is refused.
  So a run cell parked on its build cell never waits on a binding its own
  continuation would make.
- **Redundancy across siblings.** An inner body's facts whose frontier names
  only the inner rigid sit under the outer specialization and are re-solved
  once per outer binding. Sharing them would need a second parent and fork
  the chain, so the redundancy is accepted as cache the eviction policy
  prices.

## Keys: which bindings make a specialization

The variance class ([where each body interns](../../design/type-tiers.md#where-each-body-interns))
is the first input, and porosity, exposure and observed demand may enter too.
By variance alone:

- **closed** content mints nothing at run time and names no specialization:
  the root already holds it;
- **invariant** content, a capture or a parameter every tail edge passes on
  bare, a `FOR ALL` group solved to closed points, or a path class bound to a
  carrier the same each hop: the binding's digest names a shared
  specialization, the warm tier, shared across parks and across callers;
- **varying** content, a carrier minted per call, an action's result, a
  rebound parameter: no specialization. The run cell resumes the residual
  into its own scratch, the cold tier, and the parent never sees a one-off
  child.

A binding is content, digests and root handles, so a specialization reaches
nothing of the run tree. Its key digests the callee's **segment**, the index
the one shape builder gives every body it seals, in seal order, so it is the
same in every load of one text and tells apart two bodies of one text whose
captures differ in type; then the content at each **keyed name**, in slot
order: the parameters, `FOR ALL` names and `:Type` arguments any frontier at
the entry waits on, facts in nested bodies included, beside the parameters
its caching record's invariant sites read, the frame's blocks included
([`key_of`](key.rs)). A parameter keys only where its static type is a
signature or carrier type: a module's digest is the one word its content is
stored as, where a list's or a record's costs its size, so a parameter of any
other type keys nothing and the site over it routes cold. The digests a seek
computes are handed to the frame it births, which keeps each in the slot
beside the value, so the body's own demand for one — a module born over the
parameter — walks nothing
([where a digest is demanded](../../roadmap/gradual-build/digest-demand.md)).
A name a hop of the body's own tail rebinds is not
keyed, since a hop of the same callee borrows its predecessor's specialization
with no lookup; a hop into another callee seeks with the actual content, so a
callable called from sites the load cannot read — a lambda written in place,
a `MODULE` body's member — keys on its names like any other, though its mint
sites stay varying. Nothing else a frame binds enters the key, so a name no
frontier or invariant site reads costs a birth nothing, a body that keys
nothing seeks nothing, and a keyed frame seeks whatever its placement. A
capture never enters a key: `inner`'s fact on a capture `xs` waits at
`outer`'s entry, so `outer` keys on `xs`, and a fact whose frontier the chain's
keys do not cover resumes cold.

## References between the trees

- **Run to build: one counted borrow per write home born under a binding.**
  A run cell born under a binding borrows its specialization, and an `EVAL`
  of composed code borrows the composed-code cell; a tail successor of the
  same callee borrows the specialization its frame ran under, with no lookup;
  a run cell born under no binding borrows its parent's scratch, since the
  shapes and the root tier rest at `'graph` and nothing of the build root is
  read from a step; a `Shares` tenant of a keyed body borrows its
  specialization through a lend of its own, counted on the specialization
  until its host's scratch next resets after the tenant's release, since its
  host's scratch may hold nodes over the specialization's; a `Shares` call of
  a body that keys nothing, or born cold, borrows nothing; an evaluation or
  block tenant copies its asker's lend, so it reads what its frame reads; and
  a hand-off tenant copies the lend of its frame's parent, whose region it
  writes. The build tree is structural, a parent
  outliving its children, so one count covers the whole chain above. This is
  cellgraph's named lend
  ([tree cells](../../cellgraph/src/tree/README.md#death)): a creation names
  its lender and borrows its region at `'lent`, and the lender's disposal
  waits on the count, which drops at a borrower's disposal. No matrix pin runs in either
  direction: the run root pins nothing of the build tree, and a build cell is
  never pinned by a run cell. The drain resolves the borrow before the run
  cell is born, and the cell holds it, unchanged, for its life.
- **Build to run: nothing.** A build cell holds no handle to a run cell. A
  binding reaches a build cell as the key the drain hands its birth; a
  settled answer reaches a run cell through the chain alone.
- **A callable to the build cell it was born under: an opaque token.** A
  callable's closure bindings carry the build cell its birth ran under, the
  specialization of the frame that bore it or the build root, as a
  `BuildRef` ([values](../values/specialization.rs)) that only the scheduler
  and `values` open. It is not content: the closure's digest and equality
  skip it. A call seeks its frame's specialization under that cell, so
  nesting follows the lexical chain with no chain stored, and no step can
  hand the drain a build cell the scheduler did not name to it.
- **Build parent to build child: bare handles.** A table keyed by the
  binding's digest in the parent's region header, beside its type branch, as
  a type branch's digest table is: its keys are never removed, and an evicted child's entry is renewed in
  place. A bare handle holds nothing alive, and a stale one is a miss. The
  table is a small least-recently-hit cache: each entry carries the parent's
  miss count at its last hit, and a miss on a parent past its bound releases
  the least recently hit childless child before it creates the new one. A
  borrower's count does not enter the choice. An evicted child waits dead but
  undisposed until its last borrower disposes, as a released run root waits
  on its subtree, and the next miss rebuilds it.

## Types across the trees

- **The tiers, longest-lived first:** the root tier, the frozen root the
  load builds; the specialization the run cell borrows; its region branch;
  its scratch branch. A run cell's chain reads them in that order and probes
  one specialization however deep seams nest. A `Shares` run cell's scratch
  is its host's, and it reaches its host's region through that branch's link,
  which the first reading chain over the branch lays.
  [Type tiers](../../design/type-tiers.md) tells the story across modules.
- **A node lives in a tier its children outlive.** A specialization lays
  type nodes over the root tier and itself alone: a fact of a child that
  names a type its parent minted relays that type into the child's branch,
  insert-if-absent, so no branch links its parent's. A run cell's scratch tier
  may reference its specialization's nodes, as a scratch tier references the
  lent branch's today; its region reaches them only by adopt at an exit, as
  it reaches the lent branch's; and the reverse never holds. A composite a
  run cell builds over a specialization's carrier lies in the run cell's
  scratch tier.
- **A per-call type reaches a run cell's region only by adopt at an exit**,
  as a caching body's exits adopt. While the borrow stands it is read through
  the chain and copied nowhere.
- **A verdict** is recorded in the shortest-lived tier holding one of its two
  types: a verdict between root types and a specialization's types lives in
  the specialization and is evicted with it. A verdict is never load-bearing.
- **Porosity.** A sealed body's specialization may hold minted content; a
  porous body's holds only what its exits adopt out, since a porous value
  leaving the body carries its types into the region.

## Steps and the drain

- **Build cells are enterable.** The build root is entered once, through the
  scheduler's load door rather than the ready stack: its step runs the load,
  shape building, the type channel and the solver to quiescence. A
  specialization is a tree cell the drain creates and runs as it does a run
  cell: its one step writes its own handle into its parent's table, runs the
  resumption under its binding to quiescence and quiesces, staying alive with
  no continuation and reporting to nobody. What the resumption leaves parked
  is the cell's **residual**, data in its region beside the settled facts: a
  child specialization reads it through the parent region it borrows, and a
  run cell resumes it in its scratch, so no binding wakes a quiesced build
  cell. A resumption that leaves a use with no candidate ends the step
  failed, and the drain hands the fault to the asker as the frame's own.
- **The drain finds a run cell's build cell before the cell is born**, from
  the key the asker computed: the build cell the callable was born under,
  and a digest of the callee's segment with the content at its keyed names.
  The drain reads that parent's table through a read-only peek
  between steps; a live hit is the specialization, and a miss evicts where
  the parent is past its bound, then creates one
  under the parent and runs its one step at once, before anything else. The
  run cell is then created borrowing it, a `Fresh` cell as its lend and a
  `Shares` tenant through a lend of its own, and its first step lays its
  activation. A warm birth costs one peek; a cold birth one build step; no
  run cell parks for either; a hop of a tail loop of one callee borrows the
  specialization its frame ran under and looks nothing up, or its parent's
  scratch once that specialization is evicted; and a parent that has itself
  been evicted births the frame cold, with no specialization.
- **A statement seam** changes no lend: the run cell resumes the frontier
  into its own scratch, and what it borrows stands from its creation to its
  disposal.
- **The load** stands the build root up beside the run root and runs the build
  root's step to quiescence over the written program before the top-level
  run cell's first value step. The root tier is written during the load and
  never after; a specialization's branch and a composed-code cell's are
  written during the program's run.

## Refusal

Refusal is as eager as possible and lazy only where the input arrives at run
time.

- **Eager:** a closed type that does not elaborate, a shape error and an
  overlap refusal in written code refuse before anything runs. A written
  quote's code is elaborated eagerly and its refusal recorded, then reported
  where an `EVAL` runs it.
- **Lazy:** a specialization checks its binding, and a composed-code cell its
  text, when each arrives. A use left with no candidate at a resumption
  faults at the resumption point, naming the call, as an admission miss
  reads.
- A refused written program's step stops at the refusal, which is rendered
  while its names and types still stand.

## Lifetimes

| cell | lives from | to | holds |
|---|---|---|---|
| build root | the load | the program's end | the root-level table; it writes the shapes, facts, residual and root tier into the owner's storage |
| specialization | first miss on its binding | eviction, then its last borrower's disposal | settled facts, its residual, verdicts, warm types, its children's table |
| composed-code cell | the code value's birth | the value's home's death | the code's shape and facts |
| run root | the load | the program's end | top-level bindings |
| run cell | its birth | its disposal | activation, values, region branch, scratch |

## Properties

Each is a law in the dispatch suite's property tests, generated over programs
and bindings; the closed one-shot load is the oracle.

- **Monotone.** More bindings give pointwise narrower facts; no verdict
  reverts from *always* or *never*.
- **Confluent.** Binding in any order, or all at once, reaches one fixpoint:
  law 5, no answer depends on slot order, generalized to resumptions.
- **Resume equals restart.** Resuming on a set of bindings equals a fresh load
  of the program with those bindings declared from the start: a parameter's
  content written as a declared pin, a `FOR ALL` name as an alias.
- **Quiescence is exact.** A fact parks only on an unknown its rule reads;
  nothing closed is left to the run.
- **Termination.** One resumption's facts range over the types of bounded
  depth over the root's types and the binding's content, a finite set: join,
  meet and the solve never deepen a type, substitution deepens by at most
  the binding's depth at each variable position, and the facts form no
  cycle, since a parameter's and a return's static type come from the
  declaration, a local's from an earlier unit, and a mention inside a cyclic
  component is a leaf. Each re-run of a fact strictly narrows it, so one
  resumption's re-runs total at most its facts times the domain's height,
  and a varying binding never makes a
  specialization. The specializations themselves are not bounded by the
  program text: a key over invariant content the run builds, a list passed
  bare to a keyed callee, takes a new value per distinct content, so a loop
  calling such a callee over new content each iteration makes a
  specialization per iteration, and only eviction bounds how many live at
  once.

## In the literature

The driver is a monotone data-flow framework run by worklist (Kildall, *A
unified approach to global program optimization*, 1973; Kam and Ullman,
*Monotone data flow analysis frameworks*, 1977), and the residual is the
solver's answer as a constraint it could not discharge (Pottier and Rémy, *The
essence of ML type inference*, 2005). The interval reading of a static type and
the sources behind it are in [gradual typing](../../design/gradual-typing.md#in-the-literature).

## The import rule

`build` names `memory`, `scope`, `elaborate`, the scheduler's step, `values`
for a digest and `symbols`, and the type lattice. `program` and `dispatch`
name `build`: the program's bundle carries the build births and its steps
fill a specialization, and dispatch reads facts and verdicts through a run
cell's borrow. Nothing in `scope`, `elaborate` or the scheduler names `build`;
a rule reads facts through a reader the driver hands it, never through the
tree, and the scheduler hands a step its borrow's doors and no more.

## Open work

- [Evicting specializations](../../roadmap/gradual-build/specialization-eviction.md)
  — the least-recently-hit cache a build cell keeps its specializations in,
  and its bound.
- [Where a content digest is demanded](../../roadmap/gradual-build/digest-demand.md)
  — a parameter keyed by its static type, and the seek's digests kept in the
  frame's slots.
- [Monotone elaboration](../../roadmap/gradual-build/monotone-elaboration.md)
  — the worklist driver, frontiers and seams, and the properties as laws.
- [Activation typing](../../roadmap/gradual-build/activation-typing.md) — a
  frame's facts settled at its seams over the content its bindings bring.

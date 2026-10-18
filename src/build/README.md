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

This module owns the build tree and the seams the solver's frontiers decide:
the build cells, the seams that decide where a resumption lands, the
specialization tables and what they cost, and the borrow a run cell takes.
The solver's driver is the two load passes themselves, made re-entrant, so
each rule stays the code it is. Its neighbours keep what they own:
[elaborate](../elaborate/README.md) the per-expression typing rules, the type
channel's vocabulary and its walk, called from a build cell's step;
[scope](../scope/README.md) the shapes and name resolution, built by the build
root's step and stored in program storage, and the type facts' vocabulary —
unknowns, frontiers, the residual and a binding; [dispatch](../dispatch/README.md)
selection and the static pass's walk, reading verdicts through the borrow;
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
at a keyed name unparks the facts on its edge; their rules re-run, can only
narrow, and what they produce goes to the specialization the binding names. A
binding no key fixes — a statement's, a parameter's that does not key —
unparks nothing: its facts stay parked and are read per read.

The worklist's schedule is the evaluation order of the two load passes, the
type channel's walk and then the static pass's. The facts form no cycle, so
that order is a topological one: each fact is popped once, after every fact
it reads. Every read a rule makes of a load-time fact goes through a fact
store, which adds the read fact's frontier, and the read unknown where the
fact is one, to the frontier of the fact being computed; a memo the walk
keeps carries the frontier it was computed under. A fact a rule records
beside its own — a settled check, an instance read, a call by name's
contributions, a view's tree — waits on what that rule had read when it
recorded it. A resumption re-walks the
program under its binding, skipping every shape, unit and part whose
frontier the binding misses and re-running the rest, so each woken fact
re-runs once. What it re-runs is written beside the load's cells, never into
them; resumptions stack newest first, each binding extending the one before
it, and a fact one skips reads as the newest resumption below it left it, or
as the load did. A bound unknown enters at the readers alone: the type
channel's reader answers a bound name closed, a callable's own bound
`FOR ALL` name takes its binding as its group level, a bound type name seeds
as a closed type binder does, and a bound module binder's static type is
exactly its binding. Everything else follows by the rules as they are: a path
through a root whose static type is exact resolves closed.

Every lexical variable is an unknown, a `USING … SCOPE` block's surfaced names
and a quote's holes, `\` marks and `$` names among them, and so is a type binder
the type channel leaves unknown and a module binder whose declared signature
leaves a head parameter unpinned, so that a path through it is a class. A
binding removes a parameter's content, which is its path class
([paths](../dispatch/README.md#paths)), a `FOR ALL` group's solution and a
`:Type` argument at frame entry; a lexical variable's type at the statement
that binds it; and a path variable's hop at the statement that binds its
root. A surfaced name binds at its `USING` statement, which surfaces it as
the statement runs; a quote's code is the seam of its own holes,
marks and `$` names. A module binder binds to the exact type it carries: for a
parameter, the application of its declared signature the retype stamps, every
head parameter pinned. A value's carried type is never an unknown: a static
type that reads no unknown is final at the load, however wide.

An unknown is named by its **lexical address**, the segment and the
coordinate the shape binds it at, never by its lattice node, which every body
numbering its chain alike shares. A mention of a fellow member of its own
component, itself included, in the component's own statements or in a block
or `MODULE` or `GROUP` body they run inline, is no unknown: it reads the
fellow's annotation, the type its birth declares — at most `Module` for a
`MODULE` or `GROUP` body — or `[Never, Any]`, records no edge, and so reads
the same whatever order the members are typed in. A callable body nested in a
member is typed after the component and reads the fellow's fact. The facts
form no cycle.

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
  seam** settles nothing: its binding is content the run produces, an
  `EVAL`'s or an action's result, which keys nothing, so the facts waiting on
  it stay unsettled, read per read, and the run cell's lend never changes. A
  statement's binding that
  entry content determines — a `MATCH` arm's `it` over a parameter, a path
  hop rooted at one — is settled at the entry seam, since a load with that
  content declared would type it closed. A `USING … SCOPE` block's surfaced
  names are not among them: the load cannot type what a `USING` binds from
  content, so they wait at the `USING` statement and stay unsettled.
- **One build cell per seam per observed binding.** Facts that wake together,
  settle together and mint types under the same binding are one build cell
  with one type branch, never several. The build cell holds every fact any
  frontier completes at that seam, in every body under it: two bodies nested
  in `outer` that both wait on `Elt` are solved in `outer`'s specialization
  at `Elt := Number`. Frames coupled by the same rigids share a build cell,
  and one binding settles a whole section of code, the frontier class rather
  than the frame.
- **Resumption scope is per frontier.** A frontier whose members all bind at
  keyed names resumes at entry; one with a member no key covers — a mid-body
  binding, a parameter that does not key — is never resumed. The scope is
  read off where the frontier's members bind, never set program-wide.
- **The tree is a chain.** Binding order follows the lexical chain: an
  outer body's entry seam before an inner body's. A specialization's parent
  is the specialization of the enclosing seam, so every run cell's type chain
  is linear. The chain is rarely probed at run time: a settled
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
  rebound parameter: no specialization. The facts waiting on it stay
  unsettled, read per read, and the parent never sees a one-off child.

A binding is content and digests, and its types reach a specialization as
nodes the drain lays in its branch before its step
([below](#references-between-the-trees)), so a specialization reaches nothing
of the run tree. Its key digests the callee's **segment**, the index
the one shape builder gives every body it seals, in seal order, so it is the
same in every load of one text and tells apart two bodies of one text whose
captures differ in type; then the content at each **keyed name**, in slot
order: the parameters, `FOR ALL` names and `:Type` arguments any frontier at
the entry waits on, facts in nested bodies included, beside the parameters
its caching record's invariant sites read, the frame's blocks included
([`key_of`](key.rs)). A parameter keys only where its declared type is
closed and lies under `Module`, or it holds a type value: a module's digest
hashes the content it keeps and a type value's its handle, where a list's or a
record's costs its size, so a parameter of any other type keys nothing and the
site over it routes cold. A key's digests therefore cost a few hashes each,
and the frame it births keeps none of them
([`Pass::keys`](../dispatch/statics/caching.rs)).
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
keys do not cover stays unsettled.

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
- **Build to run: nothing stored.** A build cell holds no handle to a run
  cell. A binding reaches a build cell as the key the drain enters it under
  in its parent's table and as its **nodes**: the `Seek` carries the binding's
  handles beside the key — each keyed argument's carried type and, for a type
  value, the type it names — and the drain enters a missed specialization
  beside its asker (`CellGraph::enter_beside`), a hop's predecessor or else the
  cell that asked. Before the step function runs, the entry adopts each
  handle's node, children first, from the region branch of the asker's write
  home, where the birth rests and every type its arguments carry resolves,
  into the new branch, insert-if-absent; a root handle costs a bit test. The
  asker's header is read at a brand of its own, so the copy is by value by
  construction, made while the asker is between steps, and leaves no
  reference behind. A hit lays nothing: the key digests the content the
  handles are read from, so a hit's handles are the ones its miss laid. A
  settled answer reaches a run cell through the chain alone.
- **A callable to the build cell it was born under: an opaque token.** A
  callable's closure bindings carry the build cell its birth ran under, the
  specialization of the frame that bore it or the build root, as a
  `BuildRef` ([values](../values/specialization.rs)) that only the scheduler
  and `values` open. It is not content: the closure's digest and equality
  skip it. A call seeks its frame's specialization under that cell, so
  nesting follows the lexical chain with no chain stored, and no step can
  hand the drain a build cell the scheduler did not name to it.
- **Build parent to build child: bare handles.** A table in the parent's
  region header, beside its type branch and its settled facts — the header is
  `scope`'s, holding branch, table, home and facts, and `values` reads the
  branch through a projection it defines: a fixed run of entries, each a
  binding's digest, a bare handle and a stamp, laid with the cell and never
  grown. A bare handle holds nothing alive, and a stale one is a miss. The
  run's length is the parent's **bound**: the build root's is the root
  bound, every specialization's the nested bound, both parameters of the
  load (16 and 4 by default). The nested bound is a branching factor: three
  levels of bindings nested under one of the root's children hold at most
  4 + 16 + 64 live specializations by default. The root's table is shared by every top-level keyed
  callee, so its bound is wider. The root's table records the nested bound,
  and each specialization lays its own table that long.
- **The table is a least-recently-born cache.** The table counts every
  **birth** under its cell: a miss that creates a child, a seek that finds
  one live, and a hop of the same callee under one. Each entry's stamp is
  that count at its child's last birth, so no two stamps are equal. A miss
  takes the key's own stale entry, an empty one or a stale one; on a parent
  with none free it releases the child with the oldest stamp, its subtree
  with it, leaf first, before it creates the new child in that entry. A
  child with children of its own is no exception, since every child of a
  parent can come to hold one. A borrower's count does not enter the choice.
  An evicted child waits dead but undisposed until its last borrower
  disposes, as a released run root waits on its subtree, and the next miss
  on its key rebuilds it.

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
  insert-if-absent, so no branch links its parent's. A specialization's branch
  holds the nodes the drain laid from its binding and what its resumption
  minted, and nothing a frame lays: a frame lays into its region and scratch
  alone. A run cell's scratch tier
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
- **Porosity.** Sealed or porous, a body's specialization holds what its
  binding and its resumption name; a porous value leaving the body carries its
  per-call types into the region by adopt at its exit.

## Steps and the drain

- **Build cells are enterable.** The build root is entered once, through the
  scheduler's load door rather than the ready stack: its step runs the load,
  shape building, the type channel and the solver to quiescence. A
  specialization is a tree cell the drain creates and runs as it does a run
  cell: its one step lays its region header and its own table, runs the
  resumption under its binding — whose nodes the drain laid in its branch
  before the step — to quiescence and quiesces, staying alive with no
  continuation and reporting to nobody. The drain then enters it in its
  parent's table and records the parent in its header, so the drain keeps
  every table's books and no step writes another cell's region. What the
  resumption leaves parked
  is the cell's **residual**, data in its region beside the settled facts: a
  child specialization reads it through the parent region it borrows, and
  nothing else resumes it, so no binding wakes a quiesced build cell. A
  resumption that refuses ends the step failed, and the drain hands
  the fault to the asker as the frame's own.
- **The drain finds a run cell's build cell before the cell is born**, from
  the key the asker computed: the build cell the callable was born under,
  and a digest of the callee's segment with the content at its keyed names.
  The drain reads that parent's table through a peek between steps, which
  stamps a live hit; a miss on a parent with no free entry evicts first, then
  creates the specialization under the parent and runs its one step at once,
  before anything else, entered beside the asker so that the binding's nodes
  are laid in its branch before the step function runs. The
  run cell is then created borrowing it, a `Fresh` cell as its lend and a
  `Shares` tenant through a lend of its own, and its first step lays its
  activation. A warm birth costs one peek; a cold birth one build step; no
  run cell parks for either; a hop of a tail loop of one callee borrows the
  specialization its frame ran under with no lookup, stamping its entry
  through the parent its header names, or borrows its parent's scratch once
  that specialization is evicted; and a parent that has itself been evicted
  births the frame cold, with no specialization.
- **A statement seam** changes no lend and resumes nothing: no resumption
  runs in a run cell, and what a run cell borrows stands from its creation to
  its disposal.
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
  text, when each arrives. Every refusal a resumption reaches faults at the
  resumption point, worded as the load words it, since a resumption is a
  load with its binding declared: a use left with no candidate names the
  call, as an admission miss reads, and an ambiguity or a return,
  ascription, annotation or call by name that can never be satisfied reads
  as its load refusal. A frame may so fault over a path it never takes, as
  the load refuses code no run reaches.
- A refused written program's step stops at the refusal, which is rendered
  while its names and types still stand.

## Lifetimes

| cell | lives from | to | holds |
|---|---|---|---|
| build root | the load | the program's end | the root-level table; it writes the shapes, facts, residual and root tier into the owner's storage |
| specialization | first miss on its binding | its own or an ancestor's eviction, then its last borrower's disposal | settled facts, its residual, verdicts, warm types, its children's table |
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
  component is a leaf. A re-run never widens its fact, and each fact re-runs
  at most once per resumption, so one resumption's re-runs total at most its
  facts, and a varying binding never makes a
  specialization. The specializations themselves are not bounded by the
  program text: a key over invariant content the run builds, a list passed
  bare to a keyed callee, takes a new value per distinct content, so a loop
  calling such a callee over new content each iteration makes a
  specialization per iteration, and only eviction bounds how many live at
  once: no parent holds more live children than its bound.

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
name `build`: the program lists the seams once the load's passes quiesce, its
bundle carries the build births and its steps fill a specialization, and
dispatch reads facts and verdicts through a run cell's borrow. Nothing in
`scope`, `elaborate` or the scheduler names `build`; a rule reads facts
through its pass's fact store over `scope`'s vocabulary, never through the
tree, and the scheduler hands a step its borrow's doors and no more. The
region header family is `scope`'s: a half's branch, a build cell's table and
home, and a specialization's settled facts, with `values` reading the branch
through a projection it defines.

## Open work

- [Setting the specialization bounds](../../roadmap/gradual-build/bound-settings.md)
  — the bounds as a setting a user names.
- [Specialization keys over frontiers](../../roadmap/gradual-build/specialization-keys.md)
  — the key names the content at every name a frontier at the entry waits on.
- [Activation typing](../../roadmap/gradual-build/activation-typing.md) — a
  frame's facts settled at its seams over the content its bindings bring.

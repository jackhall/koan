# build

Where a program's text is decided: the load, the one walk it runs, the stores
that walk keeps its records in and the folds over them, and what a binding
the load could not see decides later — in a **settlement**, a **composition**
or a frame's **activation**.

The load decides what the text alone decides, before the program's graph
stands. A binding it could not see arrives with a frame, and the frame's facts
are then decided by the same walk under that binding: in a settlement of the
[build arena](../scheduler/README.md#the-build-arena) where the binding is
content the load names, and in the frame's own activation where it is content
the run produced. The arena is the scheduler's runtime state, its books kept by
the drain beside the cell graph, so no cell of the graph holds a fact the
build decided.

This module owns what the solver's frontiers decide and what it costs: the
load's store and a resumption's ([`Layer`](layer.rs)), the memo of what a
binding wakes ([`Wakes`](wake.rs)), the folds that lay keys, caching records
and contracts over the walk's records, and the key a settlement is found by.
The solver's driver is [the one walk](#the-one-walk): one fact function per
unit over a shape built from text alone. Its neighbours keep what they own:
[elaborate](../elaborate/README.md) the per-expression typing rules and the
walk's [type half](../elaborate/README.md#the-type-channel-at-load);
[scope](../scope/README.md) the shapes and name resolution, and the facts'
vocabulary — unknowns, frontiers, a binding, the records, the `Store` trait,
a settlement's, a composition's and an activation's facts, and the read door
`Facts`; [dispatch](../dispatch/README.md) selection and the walk's driver and
value half, and the evaluator reading what the walk recorded;
[memory](../memory/README.md) program storage, which holds the parse, the
builtin table and what the load writes for the written program;
[program](../program/README.md) the doors the drain runs a miss and an
activation through; the [scheduler](../scheduler/README.md) the arena, its
budget and its eviction.

## Where facts are decided

- **The load** runs once, before the graph stands, over the written program.
  It writes the shapes, every unit's record and every body's closing record,
  the folds over them and an index of every closed content tree they compute,
  all in program storage, and the **frozen load**'s types, laid in the
  owner's registry and read by everything after; the agent tier is laid
  beside it at the load's end.
- **A settlement** is an arena node holding one body's facts settled under
  one observed **binding** (below), each of its names bound to content the
  load names. Its walk is rooted at the body and runs over a layer on its
  parent's facts, so it decides every fact any frontier completes at that
  seam, in every body under it. Nesting follows binding order along the
  lexical chain: a binding of callable `inner`'s rigids, taken in a frame
  running under the settlement of its encloser `outer` at `Elt := Number`, is
  a child of that settlement, so a settlement's ancestors are every enclosing
  binding's. A settlement is cache: the arena may evict it once no frame holds
  it, and the next miss on its key rebuilds it, equal.
- **A composition** is an arena node holding the shape of code composed at
  run time and the facts that wait on nothing but its syntax, a chain root in
  the one-chain sense of [quotes](../scope/README.md#quotes), built from the
  code alone over a copy of its syntax and keyed on the syntax digest. The
  drain finds it by digest at each `EVAL` of the code and each call of a
  callable born in it, rebuilding it on a miss. A settlement of a body
  composed code holds is a child of its composition.
- **An activation** decides, in the frame's own region, the facts waiting on
  a binding the run produced: [below](#activations).

Each chain has one **base**: the load for a chain under the root, a
composition for one under it. Written code has one base, whose segments are
program-long; composed code adds one per distinct syntax, its segments
numbered per shape. Segments and sites compare within one base alone, and a
key under a composition digests the composition's syntax digest beside the
segment. No arena node rebuilds shapes: written code's shapes live in program
storage, composed code's in its composition. The raw parse stays in program
storage.

**Lingo.** A **type fact** is a
[load-time type](../scope/README.md#load-time-types) or verdict, and the
solver's grain is the **unit** that computes it: data the build writes in the
unit's record, in program storage for the written program, in an arena node's
bump for what a binding settles or a composition decides, and in the frame's
region for what an activation decides. An **arena node** is a settlement or a
composition; a **run cell** is a cell of the graph. An unqualified "cell" or
"node" is avoided.

## Type facts and the solver

A type fact is a static type as an interval, or a verdict, and the typing
rules are monotone: law 6 of the [lattice](../../lattice/src/types/laws.md)
makes every rule map both ends, so a fact only narrows and a verdict moves
only from *maybe* to *always* or *never*. A fact is asserted only where it
holds under every refinement of the unknowns it reads.

### The one walk

A [shape](../scope/README.md#units) is a function of the text, the builtin
table and the operator claims, and lays each body out in **units**: its
**entry unit**, which binds every slot the body holds before a statement
runs — its parameters, its own `FOR ALL` group, its `:Type` parameters, a
block's surfaced names, a code shape's captures — then one unit per statement
or component, in the order the run takes them. One **fact function** computes
every fact of a unit, in five steps:

1. **paths** — each path the unit's statements spell that its chain has not
   resolved yet, an inline root typed there;
2. **declarations** — a component of type binders, through the declaration
   door;
3. **type expressions and registrations** — each type expression and guard,
   each registration the unit binds with its overlap check, and the type of
   each callable the unit births;
4. **value statics** — each statement's node, a component of value binders
   seeded first; a quote's code the unit births is walked before them, so an
   `EVAL` reads it typed;
5. **binder facts** — each binder's static type, and each module binder's
   content tree.

A path's root lies in an earlier unit, in the unit's own component as a
fellow read at its seed, or is written in place, so every read in the five
steps is positional. An **inline body** — an arm, a block, a `USING … SCOPE`
body, a `MODULE` or `GROUP` body — runs its entry and units through the same
steps inside the unit that runs it, its facts in that unit's record. A
callable is a unit sequence of its own, walked right after the unit that
births it; a quote's code is one too, walked inside its birth unit. Each
**owning body** — the program, a callable, a quote's code — ends in a
**closing record**: its return check, exits, class, summary and recursion
flag, and for a quote's code the type a traced `EVAL` runs it at and its
typing refusal. Written code's refusal stops the walk; a quote's code that
does not type keeps its refusal in its closing record, and the `EVAL` running
it reports it.

The type half's rules are [elaborate's](../elaborate/README.md#the-type-channel-at-load),
the value half's and the driver [dispatch's](../dispatch/README.md#static-types).
A unit's facts rest in its **record**, off the shape, which the run and the
suites read through [`LoadFacts`](../scope/README.md#load-time-types). A
unit's steps walk only its own entries, which the shape lists
([`holds`](../scope/README.md#units)), and the walk logs where each unit
writes into its owning body's facts, so the unit's record lays exactly those;
an inline body's facts are laid whole as it ends. So the load is linear in a
body's size, never a scan of the whole body per unit: a path whose root is
written in place (`(ID b).Carrier`) names its statement, and a part's source
is looked for in the statement the walk is typing first, the whole body only
where a lazy read lands elsewhere.

### Recursion

A call **unfolds** into its callee's summary only where the callee's strongly
connected component, over the calls selection keeps, has closed and is
trivial. The walk computes the components by Tarjan's bookkeeping on its
demand stack: a callable enters it as its body is walked, a kept call into a
callable on the stack lowers the caller's link and unfolds nothing, a kept
call into one not yet walked walks it there, and a body whose link is its own
index closes its component, marking every member recursive where it has more
than one or calls itself. So the one record read on demand is a closing, and
whether a call unfolds never turns on declaration order. Each walk records the
recursion set of the bodies it runs: a resumption computes the components of
the bodies it re-runs, and a body it reads unchanged keeps its base's flag.
Kept calls only grow under a binding, so a cycle a binding closes passes
through re-run bodies alone, and a binding that closes a cycle unfolds no call
on it.

### The store

The walk runs over one trait, [`Store`](../scope/facts.rs): what the binding
gives an unknown, the newest record of a unit and the newest closing of a
body, each beside its frontier, whether a frontier **wakes** under the
binding, the writes of a record and a closing, and the wording of a refusal.
It interns through a chain its caller hands it. The store records no edge:
the walk reaches it only through its façade, which keeps the gathering of each
open unit. A binding miss adds the unknown to the innermost gathering, a
binding hit adds nothing, and a read of an ended record or closing adds one
read of that record's frontier, cut to the reader's chain
([the cut](#frontiers-and-seams)); only an unknown declared on the reader's
chain is gathered. A unit's **frontier** is its gathering when it ends: its
own unknowns, sorted and deduplicated, beside each frontier it read, once at
its widest cut and in the order first read ([`Frontier`](../scope/facts.rs)).
The unknowns it depends on, transitively, are its own and each read's past its
cut. A read whose cut keeps none of its frontier's unknowns is dropped, so a
frontier is empty exactly where it holds no unknown. Nothing lays them flat,
so a read costs one entry however many unknowns its frontier holds.

A read carries no binding filter: a frontier is read under the reader's
binding, which extends the binding of the layer that laid it. So a
resumption's frontier may name an unknown its own layer binds, and a store
wakes a record only on what its base leaves unbound, which an unknown already
bound never is.

Every value a record keeps is laid where it rests, through the store's `lay`,
so nothing is relayed; transient work stays in the walk's scratch.

[`LoadStore`](load.rs) is the load's, and a composition's: it binds nothing,
so every unit runs, and lays every record in the storage it is handed. A
[`Layer`](layer.rs) is a resumption's: its binding extends its base's, its
reads try its own writes and then its base's, and it wakes a record only where
the record's frontier meets what it binds that its base leaves unbound, read
once per frontier through the [wake memo](wake.rs), since frontiers share
structure. A settlement and an activation each run the walk over a layer. The
tests stand a **frame** over the production layer
([stores](../program/tests/stores.rs)), whose binding is read off its slots'
values — a type value as a type, a module by its digest tree.

### Resumption

A resumption is the walk under a binding. It visits every unit in unit order:
a unit whose stored record's frontier the binding does not wake is
**absorbed**, read as the newest layer below holds it, and every other re-runs
once, its record written beside the base's, never into it. A bound unknown
enters at the readers alone. A type-ish name — a `FOR ALL` name, a `:Type`
parameter, a surfaced name, a type binder — binds to a type, read closed in
either mode, so a type binder over it declares closed and a callable's own
bound `FOR ALL` name closes its body's type. A module binder binds to a closed
content tree beside the exact type it carries, so a view over it is settled
and a path through it resolves closed. A binding may land at a body's entry
or at a statement inside it. A refusal a resumption reaches is worded as a
load with the binding declared words it.

A resumption is **rooted** at one body: it stands the chain of bodies around
that body as the run reaches it — each enclosing unit absorbed up to the one
birthing the next — and walks the body's own units and the bodies they run.
So a settlement's or an activation's walk visits its subtree and the prefix of
its chain alone, never the whole program.

### Activations

A frame whose binding names run content — a module built at run time, a type
an `EVAL` returned, an action's result — births no settlement: the facts
waiting on that binding are decided in the frame's own **activation**. At
each **binding event** whose names the facts it reads below leave unbound —
its entry, a statement seam, a block's entry, an `EVAL`'s captures — the cell
reads the binding off its slots, by the value's digest for a module, and runs
the language's walk rooted at its body over a layer whose base is everything
it read below: the load or its composition, the settlements between, and the
events it decided before ([`activation_store`](../program/activation_store.rs)).
The walk interns through the frame's own chain, so nothing reaches the agent
tier, and the [records copier](../scope/relay.rs) lays what it wrote in the
frame's region, which the frame's evaluations and blocks read through the
door layered over it. A cell decides the events whose bind sites are its own
body's entry and statements, and reads the decided layers of every body on its
lexical chain: a block's and a module body's cells decide their own; a
callable carries the layers its encloser decided before its birth in its
closure, laid again where its closure is copied, so a call of it reads them
first.

A **decision memo** keeps what the cells of a tail chain decided, in the
region header of the half their results are built in
([`memo`](../scope/memo.rs)): up to eight entries, one per body, the least
recently used dropped. An entry is keyed by the body, the arena node the cell
runs under and the bindings of the layers beneath its first event, and holds
one decision per binding event, keyed by the event's site and the digests
bound there. A cell reuses an entry's decisions while each event's key
matches, and decides from the first that differs, so a hit checks every input
the walk reads, and a tail loop through mutually recursive bodies decides each
body once. The cells of a tail chain share the home their results are built
in, so a hop finds what its predecessor laid; a spawned call's home, where it
holds no memo, is seeded from its asker's. A half lays a bounded number of
entries, so a chain that misses at every hop stops growing its region. A property law
holds every reuse equal to a decision made from scratch.

### Unknowns

Every lexical variable is an unknown, a `USING … SCOPE` block's surfaced names
and a quote's holes, `\` marks and `$` names among them, and so is a type
binder the walk leaves unknown and a module binder: every fact reading a
module binder's content tree — a view, a path resolution, an unfold over it —
waits on it. A binding removes a parameter's content, which is its path class
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
numbering its chain alike shares. A unit's own binders are what it fixes, not
unknowns it waits on. A mention of a fellow member of its own
component, itself included, in the component's own statements or in a block
or `MODULE` or `GROUP` body they run inline, is no unknown: it reads the
fellow's annotation, the type its birth declares — at most `Module` for a
`MODULE` or `GROUP` body — or `[Never, Any]`, records no edge, and so reads
the same whatever order the members are typed in. A callable body nested in a
member is walked after the component and reads the fellow's fact. The facts
form no cycle outside a component.

The lattice is untouched: `substitute`, *fits*, `judge_by_class` and the
unifier are what the rules call, and this module lives downstream of them.

## Frontiers and seams

- **A frontier.** Each unit's record carries its frontier: the unknowns its
  facts read, and a read of the frontier of every record they read, cut to
  its chain. Frontiers share structure, a DAG at unit grain: a chain of k
  units, each reading the one before, lays O(k) entries in all. A unit whose
  frontier is empty, holding no unknown, is final at the load.
- **The cut.** A read keeps the unknowns of its frontier whose segment is at
  least the cut's: those declared in the cut's body or a body around it. It
  rests on three facts. A body's nested bodies take the segments before its
  own ([`Segment`](../scope/shape.rs)), so along one chain segments fall from
  the root inward. A frontier's unknowns lie on its record's chain. And the
  cut lies on that chain too: it is the innermost body on the reader's chain
  enclosing the record's body, and as a body leaves the chain, a read cut
  inside it is cut at its encloser instead. Segments compare within one shape
  table, and no frontier spans two.
- **A seam.** A unit's seam is the latest **bind site** among its frontier's
  unknowns ([`Shapes::bind_site`](../scope/shapes.rs)), sites ordering as the
  binding does: an outer body before an inner one, and in one body its entry
  before its units in order. A parameter, `FOR ALL` name, `:Type` parameter or
  a quote's code's capture binds at its body's entry; a local or a type binder
  at its unit; a block's parameter at the unit holding the block. No list of
  seams is stored: [the folds](#the-folds) read them off the frontiers, each
  frontier's latest bind sites and keyable names read once, through a memo
  shared by every frontier that reads it; a property law holds the memo to a
  fold over each frontier's flattened unknowns
  ([the keys fold's tests](tests/folds.rs)). An **entry seam** keys a settlement on the names its facts wait on, a
  parameter's own type aside
  ([below](#keys-which-bindings-make-a-settlement)). A **statement
  seam** settles nothing: its binding is content the run produces, an
  `EVAL`'s or an action's result, which keys nothing, so the facts waiting on
  it are decided in the frame's own
  [activation](../../design/build-storage.md#settlements-and-compositions) at
  the statement's binding event, and the arena node the run cell borrows
  never changes. A statement's binding that
  entry content determines — a `MATCH` arm's `it` over a parameter, a path
  hop rooted at one — is settled at the entry seam, since a load with that
  content declared would type it closed. A `USING … SCOPE` block's surfaced
  names are not among them: the load cannot type what a `USING` binds from
  content, so they wait at the `USING` statement and are decided in the
  activation there. A unit
  that reads an entry-bound unknown and a later-bound one seams at the later
  site, and keys nothing through it.
- **One settlement per seam per observed binding.** Facts that wake together,
  settle together and name types under the same binding are one settlement,
  never several. The settlement holds every fact any frontier completes at
  that seam, in every body under it: two bodies nested in `outer` that both
  wait on `Elt` are solved in `outer`'s settlement at `Elt := Number`. Frames
  coupled by the same rigids share a settlement, and one binding settles a
  whole section of code, the frontier class rather than the frame. A fact is
  settled once per settlement.
- **Resumption scope is per frontier.** A frontier whose members all bind at
  keyed names resumes at entry; one with a member no key covers — a mid-body
  binding, a parameter that does not key — is never resumed. The scope is
  read off where the frontier's members bind, never set program-wide.
- **The arena is a chain per frame.** Binding order follows the lexical
  chain: an outer body's entry seam before an inner body's. A settlement's
  parent is the settlement of the enclosing seam, so every run cell's facts
  read one linear chain of layers. The chain is rarely probed at run time: a settled
  fact is content, and only a verdict left *maybe* reads the chain.
- **Code finishes building before it runs.** A body's call never precedes
  the binding of a variable it captures: the shape orders each unit after
  every unit that binds a slot it reads, a deferred mention waits on its
  binder, and a component with an eager mention of its own member is refused.
  So a frame never waits on a binding its own continuation would make.
- **Redundancy across siblings.** An inner body's facts whose frontier names
  only the inner rigid sit under the outer settlement and are re-solved
  once per outer binding. Sharing them would need a second parent and fork
  the chain, so the redundancy is accepted as cache the eviction policy
  prices.

## The folds

Once the walk ends, the load folds its records ([`fold`](folds.rs)), a pure
function of the shapes and the records that a resumption reads and never
recomputes. Every input a fold needs from the lattice the walk records, so
`build` reads no type:

- **Caching.** Each minting and composite site a body records is classed by
  what its content depends on — closed; invariant where it reads only
  parameters every tail edge into its frame passes on bare and that digest at
  a constant cost; else varying — over the text's invariance and escape
  ([the shape](../scope/README.md#load-time-types)) and each site's pending
  paths read through the records' resolutions. Each body's record says
  whether its frame caches, beside its classed sites.
- **Keys**, below.
- **Contract.** Each lexical variable a contribution or a solution names is
  composed by every callable or `MODULE` body between the body reading it and
  its home, marked in that body's
  [layout](../scope/README.md#load-time-types).

## Keys: which bindings make a settlement

The variance class ([where each body interns](../../design/type-tiers.md#where-each-body-interns))
is the first input, and porosity, exposure and observed demand may enter too.
By variance alone:

- **closed** content mints nothing at run time and names no settlement: the
  root already holds it;
- **invariant** content the load names, a capture or a parameter every tail
  edge passes on bare, a `FOR ALL` group solved to closed points: the
  binding's digest names a shared settlement, the warm tier, shared across
  parks and across callers;
- **varying** content, and any content the run produced — a carrier minted
  per call, an action's result, a rebound parameter, a module built at run
  time: no settlement. The facts waiting on it are decided in the frame's
  [activation](#activations), once per binding event, and the parent never
  sees a one-off child.

A keyed name holds **load content** where a load with it declared would have
typed it: a type whose handle rests in the root, a type value naming one, or a
module whose bare member's digest names a closed tree the callee's chain
holds — the load's index under the root, and under a settlement or a
composition the chain's layers and, for a module of other composed code, that
code's composition, which the settlement then pins. A call keying anything
else seeks no settlement. A binding is content and digests, and its types
reach a settlement as an image the drain relays before its walk reads them
([below](#references-between-frames-and-the-arena)), so a settlement reaches
nothing of the run. Its key digests its base — the program's for written
code, the composition's syntax digest for composed code — and the callee's
**segment**, the index the one shape builder gives every body it seals, in
seal order, so it is the same in every load of one text and tells apart two
bodies of one text whose captures differ in type; then the content at each
**keyed name**, in slot order: each of the body's own parameters, `FOR ALL`
names and `:Type` arguments that lies in the frontier of a unit beneath the
body, its entry aside, whose seam is the entry of the body or of a callable
nested in it, beside the parameters its caching record's invariant sites
read, the frame's blocks included ([`key_of`](key.rs)). The fold lays the
list on the body's record, so a call reads its callee's keys once, off the
callee, and no shape table. A parameter's own type, which its entry unit
fixes, keys nothing: it waits on the names its declared type reads, and
whatever reads the parameter waits on them too, so a name read only at the
entry keys nothing and the parameter's type stays unsettled. A module
parameter a unit reads the content of keys like any other. Every callable keys
on its frontier names, whether or not it caches; only a caching body's
invariant sites name a warm tier, and a frame that does not cache borrows its
settlement without reading a type tier through it. A frontier whose seam is a
statement's unit keys nothing, nor does one at a quote's code entry: an `EVAL`
frame seeks no settlement, and decides its captures in its activation. So a
fact of `inner` waiting on `outer`'s `m` and its own `Elt` sits at `inner`'s
entry, `outer` keys on `m` and `inner` on `Elt`, and `inner`'s settlement, a
child of `outer`'s, lies under a chain whose keys cover the frontier. A
parameter keys only where its declared type is closed and lies under
`Module`, or it holds a type value: a module's digest hashes the content it
keeps and a type value's its handle, where a list's or a record's costs its
size, so a parameter of any other type keys nothing and the site over it
routes cold. A key's digests therefore cost a few hashes each, and the frame
it births keeps none of them. The walk records which of a callable's
parameters digest so in the callable's birth record, since `build` reads no
type, and [the folds](#the-folds) lay each callable's list once the walk ends.

A `FOR ALL` name's content is the call's solution at it, digested as the type
value the frame binds for it: a keyworded call carries the solution, an
instance holds it, and for a call by name the asker runs the solve the frame
runs at its entry. Past a header or a barrier the frame solves the node's own
group only once it has retyped or crossed the arguments, so such a call of a
body keyed on a `FOR ALL` name seeks no settlement, unless the callee is an
instance, whose solution the frame binds wherever it is called from: it is the
one birth still cold.

A name a hop of the body's own tail rebinds, passing anything but that same
parameter at it, is not keyed, since a hop of the same callee borrows its
predecessor's settlement with no lookup. That holds only where every hop
of the body into itself is a tail edge the load records, which is so for a body
whose name does not escape. A body whose name escapes — a lambda written in
place, a `MODULE` body's member, a callable read as a value — can tail-call
itself through a value the load cannot follow, so its every hop seeks with the
actual content, as a hop into another callee does; it keys on its names like
any other, though its mint sites stay varying. Nothing else a frame binds
enters the key, so a name no fact but its own type, and no invariant site,
reads costs a birth nothing, a body that keys nothing seeks no settlement, and
a keyed frame seeks whatever its placement. A capture never enters a key: `inner`'s fact on a
capture `xs` waits at `outer`'s entry, so `outer` keys on `xs`, and a fact
whose frontier the chain's keys do not cover stays unsettled.

## References between frames and the arena

- **Frame to arena: one counted borrow per run cell.** The drain resolves the
  arena node a run cell runs under before the cell is born, and the cell
  borrows it for its life: a call's frame the settlement its seek finds or
  builds, or the arena node its callee was born under where it keys nothing;
  a tail successor of the same callee, whose name does not escape, the
  settlement its frame ran under, with no lookup; an `EVAL` of composed code
  its composition; and an evaluation, a block, a hand-off and a tenant what
  its asker runs under. A hand-off successor borrows before its predecessor
  releases. A parent link holds every arena node above, so one count covers
  the whole chain. A step reads its arena node's shape and facts through its
  door at the step's own borrow and copies out indexes, digests and type
  handles alone, so no byte in a half, a parked state or a value names the
  arena, and no cellgraph lend carries a typing context.
- **Arena to frame: nothing stored.** An arena node holds no handle to a run
  cell. A binding reaches a settlement as its key and its **image**: the
  asker lays the types its keyed arguments carry that the frozen load and the
  agent tier lack into a bump the request owns
  ([`BindingImage`](../values/settlement.rs)), and the drain relays them,
  once the binding's trees resolve, into the agent tier under the root and
  into the walk's own chain under a composition. A hit lays nothing: the key
  digests the content the handles are read from, so a hit's handles are the
  ones its miss relayed. A settled answer reaches a run cell through the
  chain alone.
- **A callable to the arena node it was born under: a checked handle.** A
  callable's closure bindings carry the arena node its birth ran under, the
  settlement of the frame that bore it or the root, as an
  [`ArenaRef`](../values/settlement.rs) — a slot and its generation, which
  the arena checks at each use and which names nothing once the slot is
  reused — that only the scheduler and `values` open. A call seeks its
  frame's settlement under that arena node, so nesting follows the lexical
  chain with no chain stored. Beside it the closure holds its **stamp**
  ([`Stamp`](../scope/stamp.rs)): the requests that built each settlement of
  its chain — each one's key, its binding's handles, the content digest of
  each module it binds and, for a module of other composed code, that code's
  root. A call through a handle gone stale replays them top-down through the
  miss path, borrowing each level it reaches until the frame is born, so the
  frame runs under the settlement its callee's birth ran under, rebuilt. A
  callable born in composed code carries its code's root on its body record,
  which rebuilds the composition. Neither the handle nor the stamp is
  content: the closure's digest and equality skip both, and the stamp's
  handles count among the closure's captured types.
- **Arena node to arena node: shared holds.** A settlement's facts rest in a
  `self_cell` whose owner holds its bump beside an `Rc` of each arena node
  its records read: its parent, the composition its chain rests on, and each
  composition it **pins** — one whose closed tree its binding names a module
  by — so a step reads the chain through the newest alone, and koan writes no
  `unsafe` for it. The arena keeps the books over those holds
  ([the scheduler](../scheduler/README.md#the-build-arena)).

## Types across the arena

- **The tiers, longest-lived first:** the frozen load the load builds; the
  agent tier, where every settlement under the root rests its types; a run
  cell's region branch; its scratch branch. A run cell's chain reads them in
  that order however deep its settlements nest. A `Shares` run cell's scratch
  is its host's, and its caching chain reads its host's region between the
  agent tier and that scratch. [Type tiers](../../design/type-tiers.md) tells
  the story across modules, and [build storage](../../design/build-storage.md)
  what each tier holds.
- **No arena node holds a type a frame reads.** A settlement's or a
  composition's walk interns through a chain of its own laid in its bump,
  which no frame reads. Under the root, every type the settlement's records
  name is adopted into the agent tier, insert-if-absent, through the
  [records type visitor](../scope/record_types.rs), so it outlives the
  settlement and a frame resolves it there. A composition, and every
  settlement under one, rests nothing in the agent tier: it keeps a **term**
  of each type its shapes or records name that the root and the agent tier
  lack ([`Term`](../../lattice/src/types/term.rs)), a plain-data copy of the
  type nodes it reaches, and a frame under it mints a term into its own chain at the
  read that names it, once per binding event: every
  [reader](../scope/README.md) of a fact naming a type takes the frame's chain
  and mints inside, so no read skips it. A type composed code declares is run content: the frame
  declares it at the declaring statement, as a module's types are declared
  where it is tied.
- **The agent tier grows by load content alone.** It lays type nodes over the
  frozen load and itself alone, through the resting chain, so none references
  a branch's: a settlement's image and its records' types. It grows by each
  distinct load-content binding the program keys and is released with the
  program. No run cell interns a type into it.
- **A per-call type reaches a run cell's region only by adopt at an exit**,
  as a caching body's exits adopt, and an activation's records name only
  types that resolve in the frame's own chain.
- **A verdict** is recorded in the shortest-lived tier holding one of its two
  types: one whose types are load or agent types lives in the agent tier, and
  one over a branch's type with that branch. A verdict is never load-bearing.
- **Porosity.** Sealed or porous, the agent tier holds what a body's binding
  and its settlement name; a porous value leaving the body carries its
  per-call types into the region by adopt at its exit.

## Steps and the drain

- **The load runs before the graph.** The load door runs the load — shape
  building, the one walk, the folds and the closed-tree index — then stands
  the graph over its run root, handing every later step the frozen load, the
  agent tier and the load's facts. The root tier is written during the load
  and never after; an arena node's bump and the agent tier are written during
  the program's run.
- **The drain finds a frame's arena node before the frame is born**, from
  the request its asker hands it: the arena node the callee was born under,
  and, where the callee keys a settlement, a digest of its base and segment
  with the content at its keyed names. A live hit is stamped. A miss runs the
  fact function there and then, synchronously, through the door the program
  hands the graph ([`settled`](../program/settle.rs)): the walk rooted at the
  callee's body over a layer on the parent's facts, its records laid in the
  new settlement's bump. What its walk leaves open is the settlement's
  **residual**, the frontiers of its records beside the settled facts, which
  a child settlement reads through its parent link and nothing else resumes.
  The frame is then born borrowing it, and the drain evicts to the budget.
  No run cell parks for either, and no cellgraph door is read between steps.
- **A composition** is found by its syntax digest. The books hold digests
  alone, so on a miss the cell is born under the root, and its first step,
  holding the code value or the callee whose body record carries the code's
  root, copies the syntax and asks again with the copy, over which the drain
  builds the composition ([`composed`](../program/settle.rs)): its shapes built
  alone, walked with nothing bound, its folds, closed trees and terms laid in
  its own storage.
- **A statement seam** changes no arena node and resumes nothing in the
  arena: the frame decides it in its activation, and what a run cell borrows
  stands from its creation to its disposal.

## Refusal

Refusal is as eager as possible and lazy only where the input arrives at run
time.

- **Eager:** a closed type that does not elaborate, a shape error and an
  overlap refusal in written code refuse before anything runs. A written
  quote's code is elaborated eagerly and its refusal, or its overlap,
  recorded, then reported where an `EVAL` runs it.
- **Lazy:** a settlement and an activation check their binding, and a
  composition its syntax, when each arrives. A settlement keeps the refusal
  its walk reached, so every frame born under it faults alike, before and
  after an eviction. Every refusal a resumption reaches faults at the
  resumption point, worded as the load words it, since a resumption is a
  load with its binding declared: a use left with no candidate names the
  call, as an admission miss reads, and an ambiguity or a return,
  ascription, annotation or call by name that can never be satisfied reads
  as its load refusal. A frame may so fault over a path it never takes, as
  the load refuses code no run reaches.
- A refused written program's step stops at the refusal, which is rendered
  while its names and types still stand.

## Lifetimes

| holder | lives from | to | holds |
|---|---|---|---|
| program storage | the load | the program's end | the shapes, records, folds and closed-tree index the load writes; the frozen load beside it holds the root tier |
| settlement | the first miss on its key under its parent | its eviction, once no frame holds it and it has no child | settled facts, its residual, the closed trees it computed, and under a composition the terms its records name |
| composition | the first `EVAL` or call missing on its syntax | its eviction, once no frame holds it, it has no child and nothing pins it | a copy of the code's syntax, its shapes, facts, folds and terms |
| run root | the load | the program's end | top-level bindings |
| run cell | its birth | its disposal | activation and its decided records, values, region branch, scratch, decision memo |

## Properties

Each is a law in the dispatch suite's property tests
([resumption](../dispatch/tests/resumption.rs)), generated over programs and
bindings; the closed one-shot load is the oracle.

- **Monotone.** More bindings give pointwise narrower facts; no verdict
  reverts from *always* or *never*.
- **Confluent.** Binding in any order, or all at once, reaches one fixpoint:
  law 5, no answer depends on slot order, generalized to resumptions.
- **Resume equals restart.** Resuming on a set of bindings equals a fresh load
  of the program with those bindings declared from the start, over type and
  value facts alike: a parameter's content written as a declared pin, a
  `FOR ALL` name as an alias, a module as a `LET` of a module the load built.
- **Quiescence is exact.** A resumption re-running every unit renders as one
  waking only what its binding meets, and a frontier holds only unknowns
  declared on its unit's chain.
- **A resumption re-runs only what it wakes.** The units it re-runs are
  exactly those whose frontier at the load meets the binding, each once.
- **The rigid rule commutes with binding.** A type fact the load fixes rigid,
  substituted by the binding, is the restart's closed fact.
- **A sealed body stays sealed.** A body sealed at the load is sealed in every
  restart: a binding only closes what the load left open.
- **Every store agrees.** The load's store binding from the start, a layer
  over the load, a settlement, an activation and a frame whose slots hold the
  binding's content give equal facts: a settlement binds a module by its
  tree, as the load's store and a layer under its tree do, and an activation
  and a frame by its digest, as a layer under a digest does.
- **A rooted resumption is the program's.** One rooted at a body equals the
  program's resumption on the bodies it walks, and visits no unit outside the
  body's subtree and its chain's prefix.
- **An activation is a restart.** A frame's activation equals a restart with
  its bindings declared, and a [memoized](#activations) one equals one
  decided from scratch.
- **Levels and path identities are the shape's.** No record changes a level a
  shape numbers, and two builds of one text agree on every text fact.
- **Order does not matter.** Reordering independent declarations changes no
  fact.
- **Termination.** One resumption's facts range over the types of bounded
  depth over the root's types and the binding's content, a finite set: join,
  meet and the solve never deepen a type, substitution deepens by at most
  the binding's depth at each variable position, and the facts form no
  cycle, since a parameter's and a return's static type come from the
  declaration, a local's from an earlier unit, and a mention inside a cyclic
  component is a leaf. A re-run never widens its fact, and each unit re-runs
  at most once per resumption, so one resumption's re-runs total at most its
  units. A key names load content alone, finitely many bindings per text, so
  a loop calling a keyed callee holds O(1) settlements and the agent tier
  grows by distinct load-content bindings alone; content the run builds keys
  nothing and is decided in the frame's activation. The arena's budget bounds
  the bytes of the arena nodes no frame holds, and what frames hold is linear
  in the frame stack's distinct arena nodes and the compositions they pin.

## In the literature

The facts are a monotone data-flow framework (Kildall, *A unified approach to
global program optimization*, 1973; Kam and Ullman, *Monotone data flow
analysis frameworks*, 1977), solved in one pass in unit order rather than by
worklist, since outside a component every fact a unit reads lies in an
earlier unit; the components are Tarjan's (*Depth-first search and linear
graph algorithms*, 1972). A unit's frontier is the solver's answer as a
constraint it could not discharge (Pottier and Rémy, *The essence of ML type
inference*, 2005). The interval reading of a static type and the sources
behind it are in [gradual typing](../../design/gradual-typing.md#in-the-literature).

## The import rule

Outside doc comments and `#[cfg(test)]`, `build` names `memory`, `scope` and
`values` and nothing else in the crate ([`tests::boundary`](tests/boundary.rs)).
`program` names `build`: its load runs the language's walk into a
[`LoadStore`](load.rs) and folds the records, a settlement and an activation
run it over a [`Layer`](layer.rs), and a settlement's key is `build`'s.
Nothing in `scope`, `elaborate`, `dispatch` or the scheduler names `build`;
the walk reads facts through `scope`'s `Store` trait, never through the arena,
and the scheduler hands a step its door and no more.

## Open work

- [Setting the build budget](../../roadmap/gradual-build/bound-settings.md)
  — the budget as a setting a user names.
- [Warm composed types](../../roadmap/gradual-build/warm-composed-types.md)
  — the types a settlement under a composition derives, kept warm.
- [A birth solved once](../../roadmap/gradual-build/birth-solved-once.md) —
  a call by name keyed on a `FOR ALL` name solving its group once.
- [Settling traceable bindings at load](../../roadmap/gradual-build/load-time-settlement.md)
  — a keyed call whose sites the load can trace settled at load, leaving the
  arena compositions alone.

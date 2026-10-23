# build

The build tree: the cells the load and its resumptions run in, beside the run
tree and under the same drain.

A koan program is two trees of [cellgraph](../../cellgraph/README.md) cells.
The **run tree** is the call tree: the program's run root with every frame a
tree cell or a tenant under it. The **build tree** is where the program's
text is decided: the build root's step writes the written program's shapes,
its type facts and the frozen load's types, and a specialization holds what
a binding settles, the types and verdicts it reaches resting in the agent
tier. Its cells are
**build cells**; the run tree's are **run cells**. The
[scheduler](../scheduler/README.md) drains both, so type solving interleaves
with value computation as steps, and a run cell reads what a binding settled
through one counted borrow.

This module owns the build tree and what the solver's frontiers decide: the
build cells, the load's store, the folds that lay keys, caching records and
contracts over the walk's records, the specialization tables and what they
cost, and the borrow a run cell takes. The solver's driver is
[the one walk](#the-one-walk): one fact function per unit over a shape built
from text alone. Its neighbours keep what they own:
[elaborate](../elaborate/README.md) the per-expression typing rules and the
walk's [type half](../elaborate/README.md#the-type-channel-at-load);
[scope](../scope/README.md) the shapes and name resolution, built by the build
root's step and stored in program storage, and the facts' vocabulary —
unknowns, frontiers, a binding, the records, the `Store` trait and the read
door `LoadFacts`; [dispatch](../dispatch/README.md) selection and the walk's
driver and value half, the run reading verdicts through the borrow;
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
  the program. Its step writes the written program's shapes, every unit's
  record and every body's closing record, and the folds over them, which rest
  in program storage, and the **frozen load**'s types,
  laid in the owner's registry, read by everything after and written by
  nothing else; the agent tier is laid beside it at the load's end. Its region
  holds the root-level specialization table.
- **A specialization** is a tree cell under the build root or under another
  specialization: one **seam** under one observed **binding** (below). It
  holds every type fact the binding settled, in every body under the seam;
  the types and verdicts those settlements reach rest in the agent tier,
  which outlives it. Nesting follows binding order along the lexical chain: a
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
  specialization's key already names its segment by its index, and the
  frozen load wants one table. Written code has
  one core; composed code adds a build cell per code value.
- A specialization never rebuilds shapes: written code's shapes live in
  program storage, composed code's in its cell. The raw parse stays in program
  storage.

**Lingo.** A **type fact** is a
[load-time type](../scope/README.md#load-time-types) or verdict, and the
solver's grain is the **unit** that computes it: data the build writes in the
unit's record, in program storage for the written program and in a
specialization's region for what a binding settles, never a cell of the
graph. "Build cell" and "run cell" are the graph's, and a "node" in this doc
is the type lattice's. An unqualified "cell" is avoided.

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

[`LoadStore`](load.rs) is the load's: it binds nothing, so every unit runs,
and lays every record in program storage. The tests stand two more over the
load's records ([stores](../program/tests/stores.rs)): a **layer**, whose
reads try its own writes and then its base's, interning through the resting
chain, and a **frame**, whose binding is read off its slots' values — a type
value as a type, a module by its digest tree. Their production twins are
[the build arena](../../roadmap/gradual-build/build-arena.md)'s.

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

Nothing in production reads a resumed fact: the run reads the load's records,
and a specialization's step settles nothing until
[the build arena](../../roadmap/gradual-build/build-arena.md) runs the fact
function in a build cell.

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
  ([the keys fold's tests](tests/folds.rs)). An **entry seam** keys a specialization on the names its facts wait on, a
  parameter's own type aside
  ([below](#keys-which-bindings-make-a-specialization)). A **statement
  seam** settles nothing: its binding is content the run produces, an
  `EVAL`'s or an action's result, which keys nothing, so the facts waiting on
  it stay unsettled, read per read, and the run cell's lend never changes. A
  statement's binding that
  entry content determines — a `MATCH` arm's `it` over a parameter, a path
  hop rooted at one — is settled at the entry seam, since a load with that
  content declared would type it closed. A `USING … SCOPE` block's surfaced
  names are not among them: the load cannot type what a `USING` binds from
  content, so they wait at the `USING` statement and stay unsettled. A unit
  that reads an entry-bound unknown and a later-bound one seams at the later
  site, and keys nothing through it.
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
order: each of the body's own parameters, `FOR ALL` names and `:Type`
arguments that lies in the frontier of a unit beneath the body, its entry
aside, whose seam is the entry of the body or of a callable nested in it,
beside the parameters its caching record's invariant sites read, the frame's
blocks included ([`key_of`](key.rs)). A parameter's own type, which its entry
unit fixes, keys nothing: it waits on the names its declared type reads, and
whatever reads the parameter waits on them too, so a name read only at the
entry keys nothing and the parameter's type stays unsettled. A module
parameter a unit reads the content of keys like any other. Every callable keys on its
frontier names, whether or not it caches; only a caching body's invariant
sites name a warm tier, and a frame that does not cache borrows its
specialization without reading a type tier through it. A frontier
whose seam is a statement's unit keys nothing, nor does one at a quote's code entry: an
`EVAL` frame seeks nothing. So a fact of `inner` waiting on `outer`'s `m` and
its own `Elt` sits at `inner`'s entry, `outer` keys on `m` and `inner` on
`Elt`, and `inner`'s specialization, a child of `outer`'s, lies under a chain
whose keys cover the frontier. A parameter keys only where its declared type is
closed and lies under `Module`, or it holds a type value: a module's digest
hashes the content it keeps and a type value's its handle, where a list's or a
record's costs its size, so a parameter of any other type keys nothing and the
site over it routes cold. A key's digests therefore cost a few hashes each,
and the frame it births keeps none of them. The walk records which of a
callable's parameters digest so in the callable's birth record, since `build`
reads no type, and [the folds](#the-folds) lay each callable's list once the
walk ends.

A `FOR ALL` name's content is the call's solution at it, digested as the type
value the frame binds for it: a keyworded call carries the solution, an
instance holds it, and for a call by name the asker runs the solve the frame
runs at its entry. Past a header or a barrier the frame solves the node's own
group only once it has retyped or crossed the arguments, so such a call of a
body keyed on a `FOR ALL` name is born cold, unless the callee is an instance,
whose solution the frame binds wherever it is called from.

A name a hop of the body's own tail rebinds, passing anything but that same
parameter at it, is not keyed, since a hop of the same callee borrows its
predecessor's specialization with no lookup. That holds only where every hop
of the body into itself is a tail edge the load records, which is so for a body
whose name does not escape. A body whose name escapes — a lambda written in
place, a `MODULE` body's member, a callable read as a value — can tail-call
itself through a value the load cannot follow, so its every hop seeks with the
actual content, as a hop into another callee does; it keys on its names like
any other, though its mint sites stay varying. Nothing else a frame binds
enters the key, so a name no fact but its own type, and no invariant site,
reads costs a birth nothing, a body that keys nothing seeks nothing, and a
keyed frame seeks
whatever its placement. A capture never enters a key: `inner`'s fact on a
capture `xs` waits at `outer`'s entry, so `outer` keys on `xs`, and a fact
whose frontier the chain's keys do not cover stays unsettled.

## References between the trees

- **Run to build: one counted borrow per write home born under a binding.**
  A run cell born under a binding borrows its specialization, and an `EVAL`
  of composed code borrows the composed-code cell; a tail successor of the
  same callee, whose name does not escape, borrows the specialization its
  frame ran under, with no lookup;
  a run cell born under no binding borrows its parent's scratch, which no
  chain reads, since the shapes and the type tiers rest at `'graph` and
  nothing of the build root is read from a step; a `Shares` tenant of a keyed
  body borrows its specialization through a lend of its own, counted on the
  specialization until its host's scratch next resets after the tenant's
  release; a `Shares` call of
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
  into the agent tier, insert-if-absent; a load handle costs a bit test. The
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
  region header — the header is
  `scope`'s, holding branch, table and home, and `values` reads the
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

- **The tiers, longest-lived first:** the frozen load the build root's step
  builds; the agent tier, where every specialization's types rest; a run
  cell's region branch; its scratch branch. A run cell's chain reads them in
  that order however deep seams nest. A `Shares` run cell's scratch is its
  host's, and its caching chain reads its host's region between the agent
  tier and that scratch. [Type tiers](../../design/type-tiers.md) tells the
  story across modules, and [build storage](../../design/build-storage.md)
  what each tier holds.
- **A node lives in a tier its children outlive.** The agent tier lays nodes
  over the frozen load and itself alone, through the resting chain, so none
  references a branch's node: the drain relays a binding's nodes into it from
  the asker's region branch, insert-if-absent, and a frame under a
  specialization adopts its invariant sites' types into it. A specialization's
  region holds its settled facts and its table, and lays no type branch. The
  resumption interns into its step's scratch branch, and only what its
  records keep is adopted into the agent tier. The agent tier outlives
  every cell, so a run cell's region and scratch reference its nodes by
  covariance alone, and the reverse never holds. It grows by each distinct
  binding the program keys and is released with the program. A composite a
  run cell builds over a specialization's carrier lies in the run cell's
  scratch tier.
- **A per-call type reaches a run cell's region only by adopt at an exit**,
  as a caching body's exits adopt. While the borrow stands it is read through
  the chain and copied nowhere.
- **A verdict** is recorded in the shortest-lived tier holding one of its two
  types: one whose types are load or agent types lives in the agent tier, and
  one over a branch's type with that branch. A verdict is never load-bearing.
- **Porosity.** Sealed or porous, the agent tier holds what a body's binding
  and its resumption name; a porous value leaving the body carries its
  per-call types into the region by adopt at its exit.

## Steps and the drain

- **Build cells are enterable.** The build root is entered once, through the
  scheduler's load door rather than the ready stack: its step runs the load,
  shape building, the one walk and the folds. A
  specialization is a tree cell the drain creates and runs as it does a run
  cell: its one step lays its region header and its own table, runs the
  resumption under its binding — whose nodes the drain laid in its branch
  before the step — to quiescence over the step's scratch chain, lays the
  records it reaches in its region, and quiesces, staying alive with no
  continuation and reporting to nobody. The drain then enters it in its
  parent's table and records the parent in its header, so the drain keeps
  every table's books and no step writes another cell's region. What the
  resumption leaves open
  is the cell's **residual**, the frontiers of its records beside the settled facts: a
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
  run cell parks for either; a hop of a tail loop of one callee whose name
  does not escape borrows the specialization its frame ran under with no
  lookup, stamping its entry
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
| build root | the load | the program's end | the root-level table; it writes the shapes, records, folds and root tier into the owner's storage |
| specialization | first miss on its binding | its own or an ancestor's eviction, then its last borrower's disposal | settled facts and the warm types they name, its residual, verdicts, its children's table |
| composed-code cell | the code value's birth | the value's home's death | the code's shape and facts |
| run root | the load | the program's end | top-level bindings |
| run cell | its birth | its disposal | activation, values, region branch, scratch |

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
- **Three stores agree.** The load's store binding from the start, a layer
  over the load and a frame whose slots hold the binding's content give equal
  facts, a module bound by its digest.
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
  units, and a varying binding never makes a
  specialization. The specializations themselves are not bounded by the
  program text: a key over invariant content the run builds, a list passed
  bare to a keyed callee, takes a new value per distinct content, so a loop
  calling such a callee over new content each iteration makes a
  specialization per iteration, and only eviction bounds how many live at
  once: no parent holds more live children than its bound.

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
[`LoadStore`](load.rs) and folds the records, and a specialization's key is
`build`'s. Nothing in `scope`, `elaborate`, `dispatch` or the scheduler names
`build`; the walk reads facts through `scope`'s `Store` trait, never through
the tree, and the scheduler hands a step its borrow's doors and no more. The
region header family is `scope`'s: a half's branch, a build cell's table and
home, with `values` reading the branch through a projection it defines.

## Open work

- [Setting the build budget](../../roadmap/gradual-build/bound-settings.md)
  — the budget as a setting a user names.
- [The build arena](../../roadmap/gradual-build/build-arena.md) — build cells
  outside the cell graph, keyed on load content, with the evaluator reading
  what the build decided.

# Memory

The shape of things in storage, and no storage of its own.

The substrate is [cellgraph](../../cellgraph/README.md); this module is koan's
shapes over a cell's region, plus the things that need no cell at all. It
manages no cell storage: no frame, no region owner, no run root. A call's
storage is a cell, and its resident is a value at rest in that cell's region,
captured by the cell's continuation at the cell's own brand.
[`substrate`](substrate.rs) is the one-place spelling of every substrate name,
[`slots`](slots.rs) the layout-addressed table shape, [`knot`](knot.rs) the
index-edged group of values that refer to each other, [`bump`](bump.rs) the
arena tier outside the graph, [`program`](program.rs) the program-text owner
beside it, [`components`](components.rs) the strongly-connected-component walk
over an index graph, and [`scope_id`](scope_id.rs) the position-independent
identity a resident carries.

## Two tiers, and why the boundary falls where it does

- **The bump tier** ([bump.rs](bump.rs)) — storage outside the graph, with no
  reach and no cell, released whole when its owner drops. It holds the
  [type lattice](../type_lattice/README.md)'s registry and the scratch a caller
  passes. It is a *collections* arena: `BumpAllocator` is `&Bump`, both bumpalo's verb receiver
  and the `Allocator` its growable `BumpVec` and its hashbrown `BumpBackedMap`
  and `BumpBackedSet` are built over. A cell's `Writer` has no verb that grows a buffer, so the
  tier does not pretend to be a region, and it carries no door of its own:
  callers use bumpalo's `alloc`, `alloc_slice_copy`, `alloc_slice_fill_iter`
  and `alloc_str`. Bumpalo runs no destructor, so nothing with drop glue goes
  in; `bump_table` and `bump_set` assert that for their entries at compile time, and a
  slice or a single value is the caller's to keep `Copy`.
- **Program storage** ([program.rs](program.rs)) — program text, the raw AST,
  and what a loaded program lays down at `'graph`, in the one store it owns:
  `cellgraph`'s `Storage`, written through the same `Writer` a region is.
  `ProgramBrand::writer` is the only capability the brand exposes, so the AST,
  the builtin table and the
  [program record](../program/README.md#the-program-record) all reach storage
  by one vocabulary. The `Storage` keeps its bump private, so what goes there
  passes the `Writer`'s drop-freeness checks, and it is sound for the reason a
  `'graph` borrow is no operand: the storage outlives the graph, which prices,
  pins and reclaims none of it. Program storage is its own module because the
  parser depends on it and on nothing else here, so
  [`parse`](../parse/README.md) names `ProgramBrand` and never a cell. Keeping
  it apart from a cell's region is what lets an AST node or a builtin be shared
  by every activation without any of them being able to outlive it.

  Nothing in program storage grows in place: a `Writer` has no growable buffer,
  so a run whose length is not known up front — the
  [slot layout](../parse/builtin_shapes/layout.rs)'s entries, a group's members
  — is staged on the stack or in caller-passed scratch, ordered there, and laid
  down once at its settled length.
- **Cell storage** — everything else, laid down through cellgraph's `Writer`
  at the executing cell's own brand, `'here`. There is no run root and no
  per-call frame: what a frame shell would carry, a cell already is.

## Shapes, not instantiations

[substrate.rs](substrate.rs) holds every substrate name koan spells, in one
place, re-exported. The library takes no koan parameter — its carrier states,
its writer and its handles are generic only in the payload they carry — so
there is no koan-bound alias here; the reach width is bound once and nothing
else is. This file is the crate's **only** import of the substrate crate, so
every other module names a `crate::memory` item.

The complement of that rule matters as much. **A name that binds a *payload*
belongs with that payload, not here.** Each such binding is one line applying
a `substrate` name, so the payload's own file is where a reader finds every
state that payload travels in. Koan's data values are one such payload:
[`values`](../values/README.md) binds `Ready` to its value family, lays every
composite down through `Writer`, and prices its crossings over the placement
doors, all in its own files.

What *does* belong here is a payload-generic **shape**, because what it holds
does not enter its definition: the slot array and the knot. A façade that instantiates one at koan's own vocabulary —
a binding table keyed on koan symbols holding koan types — stays with that
vocabulary. What is *storage* in a binding table is the table shape, and that
lives here.

**No keyed table lives in a cell.** `memory` ships no name-to-value lookup
table meant to rest in a cell's region. Its hash tables, `BumpBackedMap` and
`BumpBackedSet`, cannot: a hash table allocates and grows its buckets, and a cell's `Writer`
never exposes an allocator. The [scope layer](../scope/README.md), which
owns koan's name lookup, needs none either: a body's names resolve to slot
indices where its shape is built in program storage, so a cell holds only a
slot array.

## The slot array

[`SlotArray`](slots.rs) is a fixed run of write-once binding slots laid down in
a cell's region, addressed by index rather than by key, for a table whose key
set is fixed before its first write — a call's value bindings, sized by the
body's own slot layout from [`parse`](../parse/README.md). An activation lays
down one sized run instead of building a table from nothing.

A slot is two-state: empty until its binder writes it, then bound, once.
`bind` refuses a second write as `SlotConflict`; every read goes through the
view, whose `get` hands back the value or `None`. Nothing else is recorded, because a body's
[units](../scope/README.md#units) run in its shape's order and no reader looks
at a slot before its binder has run.

The array is safe code over `cellgraph`'s once-written run
([`Writer::once_run`](../../cellgraph/src/once.rs)), which holds each value
erased to its form at `'graph` and reattaches it at the reader's brand. That is
what lets the array hand out a **`SlotView`**, the read half with no door that
binds, which is covariant in the region brand, so a reader in a shorter-lived
cell may hold one; a `Cell<Value<'cell>>` is invariant in `'cell`, so no safe
view over live slots could shorten. The array itself binds, and is invariant.
The payload is therefore a family rather than a type — the run names the
payload at any brand through it — and the view needs the family to be
[`Covariant`](../../cellgraph/src/reattach.rs). Both halves are `Copy`, so a
continuation captures either by value, and the binding vocabulary is this
file's: the run beneath it knows nothing of names or binders.

**A value at rest in the region.** `Writer` has two write verbs, `fill` and
`text`, and every simpler shape is derived here: `resident` lays one `Copy`
value down (`fill` at length one) and `collect` an exact-size run (`fill`
driven by the iterator, with no growth path). Both live beside the re-export in
[substrate.rs](substrate.rs) and are how `values` and `scope` lay down their
region-resident structs. `fill` hands back a shared `&'cell` borrow, never
`&mut`, so every write after construction goes through interior mutability.

The array's constructor takes a `Writer` and never a step context, which is how
the module stays free of the continuation family the step context is typed on.

## The knot

[`Knot`](knot.rs) is a group of values that refer to each other, laid down
together in a cell's region as one run of nodes whose sibling references are
indices into the run rather than pointers. A value is born from finished parts
([values](../values/README.md#what-a-value-is)), so a field cannot point at a
value that does not exist yet; an index exists before its node does, so a knot
closes a cycle without a placeholder. It is the shape both circular data and a
group of mutually recursive functions are born in.

**Not the type lattice's recursive group.** A sealed group in the
[type lattice](../type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)
is never stored as a run: each member is its own entry in the bump-tier
registry, keyed by the digest of `(SCC digest, index)`, and a sibling reference
is an ordinary `KType` resolved through the registry's table. Its identity is
content — a handle is lifetime-free, compares by digest, and interns equal
across declarations — where a knot's is its place in a region. The lattice has
no duplicated run to fold onto the knot, so it keeps its own addressing.

**One pointer wide.** The nodes rest behind a length header in one allocation,
written by cellgraph's `Writer::thin_run` and reached through its `ThinRun`, a
`Copy` handle one pointer wide. `Knot` is that handle, and a node is read as a
`Member` — the `(knot, index)` pair, 16 bytes with a `u32` index, so it fits
beside a tag in a 24-byte value word where a fat slice would not. The header
costs one allocation instead of two, and a resolve is arithmetic off the handle
rather than a load through a header. The raw layout that buys this cannot be
written in safe Rust, so it is a cellgraph verb pinned by cellgraph's Miri
slate, and the knot itself is safe code.

**Plan, then tie.** An `Edge` is a `u32` node index with no public
constructor. A `KnotPlan` fixes the count before the first payload and mints
edges below it; `KnotPlan::tie` consumes the plan and calls the consumer once
per node, in index order, for a finished payload. A payload whose edges name a
node not yet written is staged in the **consumer's** scratch and copied out by
the tie — no placeholder, no interior mutability — and a payload may write its
own sub-runs (a variable list of edges, say) through the same writer during the
tie, since the node run is claimed first. A knot never grows. The plan is
neither `Copy` nor `Clone`, so one plan ties exactly one knot.

**Resolving an edge.** An edge is resolved only through a member (`follow`) or
its knot (`member`), never against a bare run, and resolving one against a
knot whose count it does not fit panics like a slice index. No node holds a
pointer to a sibling, so a copy of the run carries every edge verbatim and a
copier never follows one; a copy is the consumer's, rebuilt through the
destination's writer payload by payload, and `memory` ships no crossing verb
and no weight for it.

**The case the knot cannot see.** An edge minted by one plan, placed in a
payload tied by another whose count it happens to fit, resolves to the wrong
node. That is a consumer bug of the same class as indexing one `Vec` with
another's index, documented on `KnotPlan::edge`. A brand would not close it: a
brand telling two knots apart is a fresh invariant lifetime per knot, which
cannot follow a knot resting at `'cell` into continuations, where every knot
in the cell shares one lifetime; and branding only the construction window
would need a lifetime family per payload type and still leave an edge read out
of a finished node unbranded. Consuming the plan at the tie and panicking on a
count mismatch close the case as far as it can be closed.

**Graphs built across steps.** A knot is tied once. A node of a later knot may
hold a `Member` of a finished knot as an ordinary pointer-carrying payload
field, which the knot neither mints nor sees. Knots therefore form a DAG and
cycles live only inside one: a finished knot never gains an edge to a newer
node, which is what write-once values already require.

A `Knot` equals another when they are the same run, and a `Member` equals
another when it is the same node — one knot, one index; that node identity is
all the shape says about equality. A circular data value's equality — a
bisimulation over member pairs — and a renderer that terminates on a cycle
belong to [values](../values/README.md#equality-and-rendering). Which bindings
may form a knot is delimited by a [scope's shape](../scope/README.md#visibility),
and tying a component of value binders — functions and data nodes — belongs to
[`knot`](../knot/README.md#the-tie).

## Strongly connected components

[`strongly_connected_components`](components.rs) is Tarjan's walk over an index
graph: `edges[i]` lists the nodes `i` references, and the components come back
as runs of node indices, every buffer staged in a bump the caller passes. The
emission order is reverse topological on the condensation — a component comes
out only after every component it references — which is the order a caller
that finishes each component against the ones below it relies on.

It lives here because it names nothing of what a node stands for and has two
callers above `memory` that must not depend on each other: the
[type lattice](../type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)
condenses a recursive group's members to digest them, and a
[scope's shape](../scope/README.md#visibility) condenses a body's bindings to
find the components a knot can tie and the eager cycles it refuses.

## Drop-freeness is a compile-time fact

A region releases its chunks whole and runs no destructor, so nothing stored
in one may carry drop glue. `fill` and `thin_run` check that at the door; the
slot array and the knot each restate it as a `const` assert against the cell type at its own
instantiation, so **a payload bringing drop glue with it fails the build at
the instantiation site**, not at runtime and not in review. The knot's
message names the knot, so the failure points at the shape rather than the
cellgraph verb under it.

The same discipline is why a cell's death is O(1): its region releases its
chunks rather than walking a graph.

## `ScopeId`: identity independent of placement

Pointer-derived identity couples equality to memory placement, so a relocated
or freed scope would silently break dispatch on user-declared types. A
counter-allocated newtype decouples identity from the pointer — which is
exactly why the id lives with the memory model rather than with the lexical
record it names: **what it buys is independence _from_ placement.**

Layout is `(session, idx)`. `session` is minted once per process from
entropy-derived randomness; `idx` comes from a global atomic counter. The pair
gives within-session monotonic identity and a cross-session collision
probability of 2⁻⁶⁴ — sufficient for non-adversarial use such as a
compile-then-run split where one process serializes a scope graph and another
loads and runs it.

**The counter is an identity source, not a registry.** It only ever mints:
nothing is looked up against it, no scope is reachable from an id, and the
process-wide static holds no run state — so it is not the global runtime state
this module otherwise exists to keep out. A second run in the same process
continues the counter and is none the worse for it.

## The import rule

**`memory` names no item from the rest of koan outside doc comments and
`#[cfg(test)]`**, and depends on `cellgraph` and `bumpalo` — with `hashbrown`
and `allocator_api2` named only by the bump tier, never by a cell-tier shape. It is a leaf under the rest
of the tree, and anything not on the list above is a new edge rather than a
detail.

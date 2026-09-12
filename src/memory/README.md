# Memory

The shape of things in storage, and no storage of its own.

The substrate is [cellgraph](../../cellgraph/README.md); this module is koan's
shapes over a cell's region, plus the two things that need no cell at all. It
manages no storage: no region, no frame, no owner. A call's storage is a cell,
and its resident is a value at rest in that cell's region, captured by the
cell's continuation at the cell's own brand. [`program`](program.rs) is the
program-text tier outside the graph, [`scope_id`](scope_id.rs) the
position-independent identity a resident carries, [`substrate`](substrate.rs)
the one-place spelling of every substrate name, and [`slots`](slots.rs) the
layout-addressed table shape.

## Two tiers, and why the boundary falls where it does

- **Program storage** ([program.rs](program.rs)) — program text and the raw
  AST, a bare bump outside the graph, because an AST needs no reach. It is its
  own module because the parser depends on it and on nothing else here: a
  parsed AST is bumped into this storage, so [`parse`](../parse/README.md)
  names `ProgramBrand` and never a cell. Keeping the two apart is what lets an
  AST node be shared by every activation without any of them being able to
  outlive it.
- **Cell storage** — everything else, laid down through cellgraph's `Writer`
  at the executing cell's own brand, `'cell`. There is no run root and no
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
state that payload travels in.

What *does* belong here is a payload-generic **shape**, because what it holds
does not enter its definition: the slot array, and a keyed table if one
survives beside it. A façade that instantiates one at koan's own vocabulary —
a binding table keyed on koan symbols holding koan types — stays with that
vocabulary. What is *storage* in a binding table is the table shape, and that
lives here.

## The slot array

[`SlotArray`](slots.rs) is a fixed run of binding cells laid down by
`Writer::fill` at `'cell`, addressed by index rather than by key, for a table
whose key set is fixed before its first write — a per-call frame's value
bindings, sized by the body's own slot layout from
[`parse`](../parse/README.md). An activation lays down one sized run instead
of building a table from nothing.

A cell is three-state: `Empty`, `Claimed` on the in-flight binder's producer,
or `Bound` to a payload. **One cell answers both of a name's questions** — is
it bound, and is a binder for it in flight — so a channel storing claims in
its cells needs no second structure keyed on the same name. The transitions
live on the cell itself, so a *keyed* table over the same cell type rules on a
write exactly as the array does.

Both type parameters are the embedder's: the array is the shape, and what a
bound slot holds is a choice made where it is instantiated. Its constructor
takes a `Writer` and never a step context, which is how the module stays free
of the continuation family the step context is typed on.

## Drop-freeness is a compile-time fact

A region releases its chunks whole and runs no destructor, so nothing stored
in one may carry drop glue. `fill` checks that at the door; the slot array
restates it as a `const` assert against the cell type at its own
instantiation, so **a payload bringing drop glue with it fails the build at
the instantiation site**, not at runtime and not in review.

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
`#[cfg(test)]`**, and depends on `cellgraph` alone. It is a leaf under the rest
of the tree, and anything not on the list above is a new edge rather than a
detail.

## Open work

- [Memory on cellgraph](../../roadmap/rewrite/memory-on-cellgraph.md) — the
  narrowing this document describes: the frame shell, the region owner and the
  workgraph aliases go, and the slot array moves onto `fill`.
- [Values on memory](../../roadmap/rewrite/values-on-memory.md) — the
  rewrite's foundation instantiates the shapes here.

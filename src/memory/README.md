# Memory

Where a value lives and how long — koan's instantiation of the region
substrate, and every substrate name koan spells.

The payload-generic engine underneath is a library's; this module is koan's
*policy* over it. [`region`](region.rs) declares the storage profile and the
allocation brands, [`frame`](frame.rs) the per-call frame shell,
[`program`](program.rs) the program-text tier above the run root,
[`scope_id`](scope_id.rs) the position-independent identity a resident carries,
[`substrate`](substrate.rs) the one-place alias layer, and
[`slots`](slots.rs) the layout-addressed table shape beside the keyed one.

## Three tiers, and why the boundaries fall where they do

- **Program storage** ([program.rs](program.rs)) — program text and the raw AST,
  outside the per-call tier and above even the run root. It is its own module
  because the parser depends on it and on nothing else here: a parsed AST is
  bumped into this storage, so [`parse`](../parse/README.md) names
  `ProgramBrand` and never the frame lifecycle. Keeping the two apart is what
  lets an AST node be shared by every activation without any of them being able
  to outlive it.
- **The run root** — the storage the run itself hangs off.
- **The per-call frame** ([frame.rs](frame.rs)) — one region shell per call,
  holding the **resident** it was opened around.

## A frame is a region shell and nothing else

The `Frame` shell is generic over the reattachable family it carries and
**names no koan value type**. A resident is born through `Frame::open_under`,
which takes the construction closure from the caller that knows what a resident
*is* — which is how a koan value gets into a frame without this module naming
one.

The run's own state stays out. The registries, the interner and the output sink
belong to the *run*, not to the frame that happens to be first, and live with the
run frame that adopts the run-root resident. Nothing here *builds* a koan value
either: a construction operand over a region handle belongs with the constructor
that mints it.

That leaves the import rule this module is built to keep: **`memory` names no
item from the rest of koan outside doc comments and `#[cfg(test)]`.** It is a
leaf under the rest of the tree, and anything not on the list above is a new
edge rather than a detail.

## An alias is not an instantiation, and a store is not the storage

[substrate.rs](substrate.rs) holds every substrate name koan spells, in one
place. A name arrives one of two ways: **re-exported verbatim** when the library
type takes no koan parameter (the bump doors, the family contract, the erase
primitive), or as a **koan-bound alias** when it does — `Delivered<T>`,
`Sealed<'home, T>`, `RegionHandle<'a>`, `FoldedPlacement<'b>`, `Sectioned<'a, K>`
and the rest — binding koan's witness, owner and storage profile once so no use
site restates them.

One alias per library generic, and no second alias for the same generic: a site
that needs a parameter an alias does not bind is a design question about the
profile, not a variant to add here.

This file is the crate's **only** import of the substrate library and its
allocator crates. Every other module names a `crate::memory` item, so swapping
the substrate is a rewrite of this file plus the one seam outside `memory` that
spells the substrate's own brand doors directly.

The complement of that rule matters as much. **A name that binds a *payload* —
`Delivered<SomeValueFamily>`, `Sealed<'h, SomeCallableFamily>` — belongs with that
payload, not here.** So does anything shaped by what it holds: a container
substrate and its rehoming door are cell storage and sit beside the cells. Each
is one line applying a `substrate` alias, so the payload's own file is where a
reader finds every state that payload travels in.

What *does* belong here is a payload-generic **shape**, because what it holds
does not enter its definition: the bump-backed map and the tables built over it,
the frame shell, and the layout-addressed slot array beside them. A façade that
instantiates one at koan's own vocabulary — a binding table keyed on koan symbols
holding koan types — stays with that vocabulary. What is *storage* in a binding
table is the table shape, and that already lives here.

## Two table shapes, one cell discipline

The keyed shape is the bump-backed map. The layout-addressed shape is
[`SlotArray`](slots.rs): a fixed run of binding cells in one bump allocation,
addressed by index rather than by key, for a table whose key set is fixed before
its first write — a per-call frame's value bindings, sized by the body's own
slot layout from [`parse`](../parse/README.md). An activation allocates one
sized array instead of building a table from nothing.

A cell is three-state: `Empty`, `Claimed` on the in-flight binder's producer, or
`Bound` to a payload. **One cell answers both of a name's questions** — is it
bound, and is a binder for it in flight — so a channel storing claims in its
cells needs no second structure keyed on the same name. The transitions live on
the cell itself, so a *keyed* table over the same cell type rules on a write
exactly as the array does.

Both type parameters are the embedder's: the array is the shape, and what a bound
slot holds is a choice made where it is instantiated.

## Drop-freeness is a compile-time fact

A region releases its chunks whole and runs no destructor, so nothing stored in
one may carry drop glue. The slot array's buffer wears its own `ManuallyDrop`
for the underlying bump vector's reason — its bytes are bump memory, so the vec's
destructor would only hand a bump-owned buffer back to an allocator that frees
nothing. Because that wrapper would also swallow the element proof,
`SlotArray::new` restates it as a `const` assert against the cell type directly:
**a payload bringing drop glue with it fails the build at the instantiation
site**, not at runtime and not in review.

The same discipline is why frame teardown is O(1): region bookkeeping is `Copy`
and bump-allocated, so a frame's death releases its chunks rather than walking a
graph.

## `ScopeId`: identity independent of placement

Pointer-derived identity couples equality to memory placement, so a relocated or
freed scope would silently break dispatch on user-declared types. A
counter-allocated newtype decouples identity from the pointer — which is exactly
why the id lives with the memory model rather than with the lexical record it
names: **what it buys is independence _from_ placement.**

Layout is `(session, idx)`. `session` is minted once per process from
entropy-derived randomness; `idx` comes from a global atomic counter. The pair
gives within-session monotonic identity and a cross-session collision probability
of 2⁻⁶⁴ — sufficient for non-adversarial use such as a compile-then-run split
where one process serializes a scope graph and another loads and runs it.

**The counter is an identity source, not a registry.** It only ever mints:
nothing is looked up against it, no scope is reachable from an id, and the
process-wide static holds no run state — so it is not the global runtime state
this module otherwise exists to keep out. A second run in the same process
continues the counter and is none the worse for it.

## A note on `pending_rewrite`

The runtime is this module's consumer, and the runtime is behind the
`pending_rewrite` feature. A door marked
`cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))` has no caller in a
default build until the rewrite adopts it; the marker comes off with the
adoption, and the module's own suite joins the default slate with it.

## Open work

- [Values on memory](../../roadmap/rewrite/values-on-memory.md) — the rewrite's
  foundation sits directly on this module, instantiates the frame shell, and
  re-hosts the region on a cell substrate's region directly.

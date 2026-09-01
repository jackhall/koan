# Callable cells in copied containers

Reach the closures *inside* a container the escape seam copies, so a record of
functions severs its cells' environments the way a top-level callable severs its
own ([lazy-closures.md § Lazy close](../../design/lazy-closures.md#lazy-close-the-copy-verb-through-callables)).

**Problem.** A **callable cell inside a copied container** rides verbatim. The
`Consolidate` verb fires at exactly two surfaces — a top-level `KFunction`
crossing the priced escape seam and an explicit `CLOSE OVER` capture
([kobject.rs](../../src/machine/model/values/kobject.rs)) — and a cell reached
through a container's deep copy is neither, so a record of closures crossing the
seam severs nothing for its cells and keeps every producer region they name.

The pricing cannot correct that on its own, because it cannot see the cost. A
container's memoized copy weight counts its substrate bytes and no environment,
so the chooser reads a callable-bearing record as cheap to copy and cheap to
pin alike; even a consolidation that would release the whole producer chain is
never offered, since nothing in the comparison knows the chain is there.
Accounting for those environments and reaching the cells are therefore one
piece of work: a cost term for something the copy still cannot do would price a
decision that is never taken.

**Acceptance criteria.**

- Deep-copying a container consolidates its ready callable cells, and the
  copied container's cells reach no source region.
- The cost the pricing chooser reads for a container accounts for its callable
  cells' environments, so a record of closures is priced against what pinning it
  would retain rather than against its substrate bytes alone.
- A cell whose environment the engine cannot rebuild rides verbatim inside the
  copied container, exactly as an unready top-level callable does — the copy is
  never partial in a way that leaves a cell naming a dead region.

**Directions.**

- *Environment cost in container memos — open.* Fold a callable cell's chain
  cost into the substrate's memoized copy weight at section time (stale once
  the scope grows — a monotone under-count), or read it live at the seam for
  callable-bearing containers only.
- *Where the cell's copy fires — open.* Recurse the `Consolidate` verb through
  the container's relocation fold, so a cell takes the same nested
  `transfer_into` a captured callable takes, or keep the verb top-level and give
  the container copy its own callable arm.

## Dependencies

The sibling item [Callable copy tuning](callable-copy-tuning.md) owns the
pricing constants this item feeds a new term into.

**Requires:** none — the container copy and the callable copy it joins are both
shipped.

**Unblocks:** none tracked yet.

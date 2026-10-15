# A counted borrow of a named cell

A cell borrowing another cell's region for its life, with the lender's
disposal waiting on the count.

**Problem.** A tree cell borrows exactly one thing: its tree parent's write
home's scratch, at `'lent`, from its creation to its death
([tree cells](../src/tree/README.md#death)). The lender is derived, never
stored (`Cells::lender`, [graph.rs](../src/graph.rs)), so a creation cannot
name one. The count, `Tenancy.scratch_borrowers`
([tenant.rs](../src/tenant.rs)), holds off the lender's scratch reset and
nothing else: a lender released while borrowed disposes as soon as its
children and tenants have, and the count drops at the borrower's release,
while a dead-but-undisposed borrower's scratch, which may name the lent
bytes, lives until its disposal. No door reads a cell's region header
outside `enter`, so an embedder's drain cannot look a cell up between steps.
Koan's [build tree](../../roadmap/type-tiers/build-tree.md) needs a run cell
to borrow a cell of a second tree under the same drain, and its drain to
find that cell before the borrower is born.

**Acceptance criteria.**

- `Lend` names what a tree cell's creation borrows: its parent's scratch, a
  named slab or tree cell's region, or another cell's lend; it is stored on
  the tree cell, and `create_tree` keeps its signature and its behaviour
  beside `create_tree_lent`.
- A step in a cell with a named lend is handed the named cell's region
  writer and header at `'lent` through `lent_region`, and one lent a scratch
  is handed it through `lent_scratch`, each `None` otherwise, with `'lent`'s
  relations unchanged; a tenant's step is handed its host's lend.
- A lender's disposal waits on its region borrowers, and its scratch reset on
  its scratch borrowers: both counts drop at a borrower's disposal, never at
  its release; a dead lender off the borrower's chain disposes when its last
  borrower does, and one on the chain disposes once.
- A named lend of a dead or stale cell is refused; a lend copied from a cell
  whose lender is dead but undisposed is taken, since that cell's count holds
  the lender; a copied scratch lend is refused under any parent but its
  lender; a named lend mints no pin row.
- `peek` reads a live slab or tree cell's region header outside any step,
  under a brand quantified per call, hands the header's value to a closure
  and never a door, and refuses a dead, stale or tenant handle.
- `is_childless` answers whether a tree cell has undisposed children.
- The surface test names every new verb; the Miri slate pins the named
  lend's region read at `'lent` and the peek.

**Directions.**

- *When the count drops* — decided. At the borrower's disposal: a released
  borrower's scratch survives to its disposal and may hold a `'lent`
  reference into the lender.
- *How a copied scratch lend is counted* — decided. It is refused under any
  parent but its lender, so every scratch borrower is its lender's child,
  whose child count already holds the lender's disposal. The build tree's run
  cells borrow regions, never a build cell's scratch.
- *The doors a step is handed* — decided. One per half, so a step names
  which it writes into: a lent scratch is reclaimed after its borrowers
  leave, while a write into a lent region stays for the lender's life.
- *Re-lending a live cell* — deferred. A `relend` verb must refuse while the
  cell's scratch state is parked, its scratch held or a tenant's scratch
  state parked; it is
  [monotone elaboration](../../roadmap/gradual-typing/monotone-elaboration.md)'s,
  for a seam mid-body.

## Dependencies

**Requires:** none — a substrate change.

**Unblocks:**

- [The build tree](../../roadmap/type-tiers/build-tree.md) — a run cell's
  borrow of its specialization, and the drain's lookup before the birth.

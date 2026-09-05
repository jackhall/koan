# The cell substrate

**Problem.** [design/cellgraph.md](../design/cellgraph.md) and
[design/liveness-matrix.md](../design/liveness-matrix.md) describe a crate
that does not exist: there is no `cellgraph/Cargo.toml`, no workspace member,
and no source file. The repo's tooling knows two doc trees only —
`tools/doclinks.py` hardcodes the koan and `workgraph/` roots, so nothing
under `cellgraph/` is link-checked, orphan-checked, or counted in dependency
symmetry — and `tools/verify.sh` picks its slate from `workgraph/` paths, so a
cellgraph-only change has no slate to run. The cell model — a slab slot with a
generation, an optional erased continuation, a parent chain, the `create` /
`enter` / `release` verbs — has no implementation. The erase-store / witness /
reattach core, the `Region` bump allocator, and the sealed / opened carrier
states exist only inside `workgraph`'s witnessed module
([witnessed-memory.md](../../workgraph/design/witnessed-memory.md)), where
reach is an owned pin bundle with a `PinsRegion` hook and an antichain fold
([reach.md](../../workgraph/design/reach.md)) rather than a slab bitmask. And
a cell released while still held has nowhere to go: the sealed tier of
[liveness-matrix.md § The sealed tier](../design/liveness-matrix.md#the-sealed-tier)
— frozen aggregates, monotone sealed ids, the reverse naming index, the
accessor — is unimplemented, so masks have no sealed half.

**Acceptance criteria.**

*Skeleton.*

- `cellgraph/Cargo.toml` and `cellgraph/src/lib.rs` exist, the crate is a
  workspace member and default member, and `cargo check -p cellgraph`
  passes with the crate's module doc naming the two design docs.
- `cellgraph` depends on neither `workgraph` nor `koan`, and no other
  workspace crate depends on `cellgraph` yet.
- `tools/doclinks.py` treats `cellgraph/design/` and `cellgraph/roadmap/`
  as a third root: broken links, orphans, and `Requires:`/`Unblocks:`
  symmetry are gated for the tree, and `cellgraph/roadmap/README.md` carries
  a `sync-next`-owned `## Next items` slice.
- `tools/verify.sh` detects a cellgraph-only change scope and runs a
  cellgraph slate (`cargo test -p cellgraph`, clippy on the same, the
  doc-link check), reporting `workgraph` and koan compile state as
  information.
- A `cellgraph/observe/miri_slate.md` log exists, and the `miri` skill's
  command of record accepts `-p cellgraph`.

*Cell table.*

- A `CellTable` owns a slab fixed at a cap given at construction; `create`
  returns a `Copy` handle (slot plus generation) or a refusal value when the
  slab is full, and never grows past the cap.
- Every operation on a handle whose generation does not match the slot's
  current occupant returns an error; a test creates, releases, re-creates
  into the same slot, and observes the stale handle rejected.
- `create` accepts an optional parent handle; the new cell's birth row is
  the parent's birth row plus the parent's bit, and a test asserts the
  containment invariant (a cell's birth row contains its parent's) across a
  chain of depth three after the middle cell is released.
- `enter` runs a closure with the cell's continuation moved out and
  re-anchored at the closure's lifetime, marks the cell executing for the
  closure's duration, and rejects a nested `enter` on the same cell; the
  closure may store a successor continuation, and a cell created without a
  continuation is enterable as storage-only.
- The continuation family is an embedder type parameter constrained by the
  reattachable contract; the table stores it erased and never calls it.

*Witnessed core.*

- Each live cell owns a bump region in pointer-stable chunks, minted lazily
  at the cell's first allocation; a value allocated into it is a witnessed
  carrier whose reach is a fixed-width bitmask over slab slots.
- The pin matrix exists: minting a value into cell M performs
  `column[M] |= mask & !bit(M)`, and no public path stores a value into a
  region without passing through that mint. A `compile_fail` doctest shows a
  loose value-plus-mask pair cannot be placed.
- A step context obtained through `enter` allocates into the executing cell,
  allocates into any other live cell by handle (destination-homed placement,
  minting into that cell's column), and mints a bare hold on another cell.
- `release` reclaims a cell exactly when its pin row, birth row, and
  executing bit are all clear; the column clear cascades, and a property
  test over random create / mint / release interleavings observes no cell
  reclaimed while any mask names it and no cell resident once nothing does.
- The erase / witness / reattach core, the `reattachable` macro, and the
  sealed / opened carrier states are ported from `workgraph`'s witnessed
  module with their `unsafe` retype confined to the same single site;
  `workgraph`'s copies are untouched.
- A debug-mode ring detector walks the hold graph from a given cell and
  reports a cycle; it is not consulted on any mint path.

*Sealed tier.*

- Releasing a cell with a nonzero pin row seals it: its column freezes into
  an aggregate, its chunks detach unmoved, its slot recycles under a fresh
  generation, and it takes a never-reused sealed id.
- A value's reach is a hybrid mask: slab bits plus a sparse sealed-id set;
  the mint folds the sealed part into the destination's sealed-hold set, and
  union remains OR plus set union.
- The seal transition rewrites every live holder's stored masks that name the
  dying slot to the sealed id, moves each frozen aggregate's bit to the id
  through the reverse naming index, and touches no byte of the sealed
  region's own storage; a test seals a region with a large resident value set
  and asserts the transition's work is bounded by the row and the index, not
  the storage.
- A sealed region's holder count reaches zero only through a holder's own
  release or reclamation, and it is then reclaimed with its aggregate
  released, cascading.
- Reading a value out of a sealed region is possible only through a step
  context inside `enter`, and the value comes back with reach derived as the
  region's sealed id plus its aggregate.
- A property test over random create / mint / release / read interleavings
  asserts mask validity — every readable bit or id is covered by a live
  column or a frozen aggregate — and that no slot recycles before its
  occupant's seal or reclamation completes.
- Every `unsafe` site in the crate is on the crate's Miri slate, and the
  slate is clean.

**Directions.**

- *Crate name — deferred.* `cellgraph` stays the working identifier; the
  final name is settled with
  [workgraph-extraction.md](../../workgraph/roadmap/workgraph-extraction.md)'s
  naming pass.
- *Edition and lints — decided.* Match `workgraph`: edition 2024,
  `publish = false`, the same clippy configuration.
- *Birth-row storage — open.* (a) a bitset row per slot in a second matrix,
  the shape the design describes; (b) a parent handle per slot plus a derived
  chain-holder count. Recommended: (a) — one representation for both
  relations, and the containment invariant is a row comparison.
- *Continuation slot — decided.* One `Option` of erased continuation per
  slot; `enter` takes it, the step may put one back. No queue of pending
  continuations: a cell that wants a queue keeps it in its region.
- *Mask width — decided.* Fixed at the slab cap, as `[u64; CAP / 64]`
  words; the cap is a construction parameter of the table, not a compile-time
  constant, so the mask is a small heap or arena word slice.
- *Delivered envelope — decided.* Not ported. There is no in-flight habitat;
  a value crosses cells inside a step.
- *Sealed-set representation — open.* Sorted small-vector of ids versus a
  hash set. Recommended: sorted small vector; sealed sets are sparse and
  union is a merge.
- *Reverse naming index — decided.* Per slot, a sparse set of sealed ids,
  registered at seal and unregistered at reclamation, as the design states.
- *Group sealing — deferred.* Sealing a dying call subtree as one record is
  design work first; it lands, if at all, with
  [absorption.md](absorption.md).

## Dependencies

The item lands in four phases — skeleton, cell table, witnessed core, sealed
tier — each ending in its own commit; the phase plan is
`scratch/cell-substrate-plan.md`.

**Requires:** none — foundation.

**Unblocks:**

- [Absorption](absorption.md) — every merge is a seal-transition variant.
- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — the pull shape of delivery needs the accessor.

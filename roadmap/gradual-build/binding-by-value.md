# The binding by value

A specialization holds the nodes of the binding it was built over.

**Problem.** A `Seek` carries a build cell, a key and the specialization's
work (`src/scheduler/seek.rs`), and the key is digests alone: a handle is the
content digest of its node (`lattice/src/types/handle.rs`), one-way, and a
specialization's chain runs through its own branch and the build root
([references between the trees](../../src/build/README.md#references-between-the-trees)).
A binding whose type the asker minted outside the root — a module built at
run, a type value composed in a frame's scratch — reaches the specialization
as a handle no tier in its chain resolves. The drain creates a missed
specialization and steps it at once (`seek` in `src/scheduler/drain.rs`)
laying nothing into its branch, which fills only as its borrowers lay into it.

**Acceptance criteria.**

- A `Seek` carries the binding's handles beside its key.
- Between creating a missed specialization and stepping it, the drain lays
  each handle's node from the asker's chain into the new branch,
  insert-if-absent, so the step's chain resolves every handle of the binding
  through the specialization's own branch.
- A specialization's branch holds no reference into a run cell: the copy is
  by value, made while the asker is between steps.
- A frame born under a specialization finds every handle of its binding in
  its lend's chain, whether its seek missed or hit.

**Directions.**

- *The copy* — decided. `Content::transfer`
  (`lattice/src/types/transfer.rs`) re-lays a node from the asker's chain
  into the specialization's branch with both halves held, as a crossing
  relays a value's types
  ([types across the trees](../../src/build/README.md#types-across-the-trees));
  a node already in the root or the branch is left as found.
- *What the key is* — decided. Unchanged: a handle is already the content
  digest of its node, so the key digests the same content it did.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — the resumption reads its
  binding's types through the specialization's own chain.

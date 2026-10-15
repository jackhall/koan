# The build image

What a program's text decides, written out once and read back by every run.

**Problem.** Every run of a program starts from its text.
[`CellSubstrate::load`](../../src/program/substrate.rs) parses, builds every
shape, runs [the type channel](../../src/elaborate/README.md#the-type-channel-at-load)
and the static pass, and stands the graph up, all inside one process, and
nothing of that work outlives the process: program storage, the frozen type
root and — once [the build tree](../type-tiers/build-tree.md) ships — the
specializations settled under each observed binding are released when it
exits. A program run twice loads twice, and a warm tier a long run built is
cold again at the next start. Nothing in the process has a form outside it:
a reference into program storage or a region is a native `&'graph T` or
`&'cell T`, a handle is a slab slot plus a generation
([handles](../../cellgraph/src/handle.rs)), and a region is a
[bumpalo](../../cellgraph/src/region.rs) chain of chunks, so a pointer's bits
name nothing in another process. What is already stable across processes is
every *key*: a type is its content digest and the lattice's tables hash by
that digest's own word ([`slot_hash`](../../lattice/src/types/tiers.rs)), a
specialization is keyed by content digests, and a body by a segment index
the shape builder assigns the same way in every load of one text. Nothing
turns the things those keys name into an image, and nothing resumes from one.

**Acceptance criteria.**

- A build of a program writes an **image**: program storage, the type root and
  every cell of the build tree at quiescence, each region laid out as one
  contiguous range, with every reference serialized as a pair — the ordinal
  of the region's cell in the image, the offset into that range — and every
  handle as its cell's ordinal. The image carries the source text's digest.
- A run started from an image lays each region back as one chunk, relocates
  every serialized reference and handle, and steps the top-level run cell
  without parsing, building a shape or running a load pass; a test observes
  that the load passes do not run and that the program's output equals a cold
  run's.
- A specialization in the image is hit by the first call under its binding in
  a run started from it; a test observes that the binding's warm tier is not
  rebuilt.
- The image closes over itself: every serialized reference names a cell in
  the image, so the build tree's rule that no build cell holds a handle to a
  run cell is what makes the image complete.
- Relocation is a property of the types laid, enforced where drop-freeness is
  today: a value laid in program storage or in a build cell's region
  implements the relocation walk the `Writer` requires, as its lay verbs
  require `!needs_drop` ([`Writer`](../../cellgraph/src/region.rs)) — and a
  Rust function pointer, such as a
  [builtin shape's](../../src/parse/builtin_shapes/binder.rs), serializes as
  the builtin's index, never as an address.
- An image whose source digest is not the program's is refused with an error
  naming the mismatch, and never resumes.

**Directions.**

- *The serialized reference — decided.* A reference is `(cell ordinal,
  offset)` and a handle is a cell ordinal, computed when the image is written
  by walking each region's chunks and assigning them consecutive ranges. Live
  references stay native `&'cell T`: an offset-based pointer at run time
  would trade the brand-and-lifetime discipline every region read rests on
  for an indirection on every read, to save work that happens once per
  image.
- *What the image holds — decided.* Program storage, the type root and the
  build tree, and never a run cell: the build tree is by design what the text
  decides, and the program's values are the run's. A table in the image
  hashes by a content word — a `DigestTable`, the tiers' `slot_hash` — never
  by a process-seeded hasher; a seeded `hashbrown` table is rebuilt at load or
  kept out of the image.
- *Holding run cells whose inputs are external and running the rest at build
  time — open.* A build could also step run cells whose inputs are known and
  hold those that read input, a file or the network, so a constant-folded
  value and every error that does not depend on an input surface before the
  program runs. (a) Out of this item's scope: the image is the build tree, and
  partial evaluation of the run is an item of its own once the image exists.
  (b) In scope, with the set of held cells enumerated here. Recommended: (a);
  what the text decides already has a tree, and what a run decides does not
  yet have a representation that outlives it.
- *What an editor reads — open.* A site's static type and a keyworded use's
  selected overload are in the image. (a) An editor reads them from the
  image through a query door over a loaded image. (b) An editor loads the
  program and queries the live substrate, and the image is a run-time cache
  only. Recommended: (a), since the image is the one representation that
  exists without running anything.
- *Code generation over the image — open.* The image is also what a code
  generator would consume, emitting direct calls where the build resolved a
  dispatch and table dispatch where it did not. Whether that is a back end
  over this item or a separate item, and what its first target is, is open;
  the image's form is not reopened by it.

## Dependencies

**Requires:**

- [The build tree](../type-tiers/build-tree.md) — the tree the image
  serializes, and the segment index that names a body across processes.

**Unblocks:** none — a leaf.

# The build image

What a program's text decides, written out once and read back by every run.

**Problem.** Every run of a program starts from its text.
[`CellSubstrate::load`](../../src/program/substrate.rs) parses, builds every
shape, runs [the one walk](../../src/build/README.md#the-one-walk) and its
folds, and stands the graph up, all inside one process, and nothing of that
work outlives the process: program storage, the frozen type root and the
agent tier are released when it exits. A program run twice loads twice.
Nothing in the process has a form outside it: a reference into program
storage or a region is a native `&'graph T` or `&'cell T`, a handle is a slab
slot plus a generation ([handles](../../cellgraph/src/handle.rs)), and a
region is a [bumpalo](../../cellgraph/src/region.rs) chain of chunks, so a
pointer's bits name nothing in another process. What is already stable across
processes is every *key*: a type is its content digest and the lattice's
tables hash by that digest's own word
([`slot_hash`](../../lattice/src/types/tiers.rs)), a layer is keyed by
content digests, and a body by a segment index the shape builder assigns the
same way in every load of one text. Nothing turns the things those keys name
into an image, and nothing resumes from one.

**Acceptance criteria.**

- A build of a program writes an **image** in two layers. The **cold layer**
  is what the text decides: program storage — the shape, the folds, the
  closed-tree index and every layer the load laid
  ([traced layers at load](traced-layers-at-load.md)) — and the type root
  above [the builtin image](../../src/builtin_image.rs). The **warm layer** is
  what one run observed and may be left out: what
  [the agent tier keeps](agent-tier-keeps.md) and the compositions live in
  the arena; a decision resting in a run cell is never in it. Each bump is
  laid out as one contiguous range, every reference serialized as a pair —
  the ordinal of its storage in the image, the offset into that range — and
  every arena handle as its node's ordinal. The image carries the source
  text's digest.
- A run started from an image lays each region back as one chunk, relocates
  every serialized reference and handle, and steps the top-level run cell
  without parsing, building a shape or running the load's walk; a test
  observes that the walk does not run and that the program's output equals a
  cold run's.
- A run started from an image with a warm layer finds what it holds at first
  use; a test observes that a composition in it is not rebuilt and what the
  agent tier keeps is not relaid.
- The image closes over itself: every serialized reference names storage in
  the image, so the build's rule that no arena node holds a handle to a run
  cell is what makes the image complete.
- Relocation is a property of the types laid, enforced where drop-freeness is
  today: a value laid in program storage or in an arena node's bump
  implements the relocation walk the `Writer` requires, as its lay verbs
  require `!needs_drop` ([`Writer`](../../cellgraph/src/region.rs)) — and a
  Rust function pointer, such as a
  [builtin shape's](../../src/parse/builtin_shapes/binder.rs), serializes as
  the builtin's index, never as an address, and a reference into the builtin
  image as the type's handle, resolved by a probe when the image is read.
- An image whose source digest is not the program's is refused with an error
  naming the mismatch, and never resumes.

**Directions.**

- *The serialized reference — decided.* A reference is `(cell ordinal,
  offset)` and a handle is an arena node's ordinal, computed when the image is
  written by walking each bump's chunks and assigning them consecutive
  ranges. Live references stay native `&'cell T`: an offset-based pointer at
  run time would trade the brand-and-lifetime discipline every region read
  rests on for an indirection on every read, to save work that happens once
  per image.
- *What the image holds — decided.* The cold layer always, the warm layer at
  the writer's option, and never a run cell: program storage and the root are
  what the text decides, the warm layer what one run observed, and the
  program's values are the run's. A table in the image hashes by a content
  word — a `DigestTable`, the tiers' `slot_hash` — never by a process-seeded
  hasher; a seeded `hashbrown` table is rebuilt at load or kept out of the
  image.
- *When the image is written — open.* The cold layer at the load's end, by a
  build that runs nothing; the warm layer at a run's exit, over the cold
  layer the run started from. Whether one command writes both, and whether a
  warm layer from one run is merged into the next's, is open.
- *Stepping the run at build time — deferred.* A run cell whose inputs are
  known stepped at build time is
  [partial evaluation](partial-evaluation.md), its own item over this one.
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

The [build arena](../../src/scheduler/README.md#the-build-arena) it serializes
holds compositions alone once settlements are retired, program storage holds
every layer the load laid, and the [builtin image](../../src/builtin_image.rs),
the lower layer of the root, is what it stubs a reference into by handle.

**Requires:**

- [Settlements retired](settlements-retired.md) — the arena it writes holds
  compositions alone, and program storage every layer the load laid.
- [What the agent tier keeps](agent-tier-keeps.md) — which of the tier's
  content is the cold layer's and which the warm's.
- [A refused program stays loaded](refused-programs-stay-loaded.md) — program
  storage held apart from a graph, which a run from an image stands up before
  its graph.

**Unblocks:**

- [Partial evaluation of the top level](partial-evaluation.md) — a value a
  build decided needs an image to be kept in.

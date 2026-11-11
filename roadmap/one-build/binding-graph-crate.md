# The binding graph crate

[The binding graph](../../design/binding-graph.md) as a closed abstraction
beside the build, held to its laws before the build reads it.

**Problem.** The build never stores which bind site owns a question: "no
list of seams is stored"
([frontiers and seams](../../src/build/README.md#frontiers-and-seams)), and
each reader derives the part it needs. The keys fold reads each frontier's
latest bind sites; [the trace](../../src/build/README.md#the-trace) keeps
slot maps per call site and follows a name only where it is passed on bare;
the decision memo keys an entry by body, depot slot and the digest of a
spine ([activations](../../src/build/README.md#activations)); a settlement's
parent link repeats the lexical chain. The data that moves, closes, keys and
memoizes unit records in [scope](../../src/scope/README.md#load-time-types)
is several times the size of the data that states a fact, and none of it can
be tested apart from koan's walk, shapes and lattice.

**Acceptance criteria.**

- A crate `bindgraph` holds `BindingNode`, `SiteQuestion`, `SiteAnswer`, the
  `BindsAfter`, `PassesName` and `AnswerBranch` edges, the `OpenCaller` mark
  and `BoundInstance`, as
  [the design](../../design/binding-graph.md#vocabulary) names them, and
  depends on no koan code: a path into koan is a build error.
- Nodes, questions and edges are laid from a description of the text alone,
  and a `SiteQuestion`'s owner is the same under every binding; instances and
  answers are made under a binding and change no node.
- The crate is generic over a fact domain trait: what a `SiteAnswer` and a
  `NameQuestion` are, when a binding keeps a `NameQuestion`'s answer, how a
  `PassesName` mapping is applied to a binding, whether a composed mapping
  grows, and the order under which an answer narrows. Its tests supply a
  small domain of their own.
- A `PassesName` edge carries a mapping — the caller's name itself, a type
  built over the caller's names, or fixed content — and making a caller's
  `BoundInstance` makes the callee's under the mapped binding, which may
  leave caller's names open.
- A cycle of `PassesName` edges whose composed mapping grows is refused when
  the graph is laid, before anything is bound.
- A `BoundInstance` is found by the answers its `NameQuestion`s took, never
  by a value, and is reused by any binding that keeps them.
- Each law in [the design](../../design/binding-graph.md#the-crate) is a
  property test over generated graphs and bindings, with the same graph
  answered from scratch under the whole binding as the oracle.
- Koan's build is unchanged, and no koan module imports the crate.

**Directions.**

- *What answers a question — decided.* The embedder hands the crate one
  function that answers a `SiteQuestion` under a binding and reports the
  `NameQuestion`s it asked; the crate decides ownership, edges and reuse, and
  never an answer's content.
- *How an `AnswerBranch` is discovered — open.* The embedder's function
  reports the bind sites an answer adds, and the crate lays them as it goes;
  or the embedder lays every branch the text can take up front and an answer
  selects one. The first matches how the walk finds a kept callee; the second
  keeps the graph a function of the text, as the design states it.
- *Where a `BoundInstance` rests — deferred.* The crate lays through a
  storage door its embedder supplies;
  [the build over the binding graph](build-over-the-binding-graph.md)
  chooses the storage.

## Dependencies

**Requires:** none — a foundation.

**Unblocks:**

- [The build over the binding graph](build-over-the-binding-graph.md) — the
  build stores and reads the crate's graph.

# The build over the binding graph

Koan's build storing [the binding graph](../../design/binding-graph.md) and
every reader of an answer reading it there.

**Problem.** One function, a body's facts under a binding, has five homes
with five lifetimes and keys: the load's base layer, a layer the load lays
for a traced binding, a settlement in the depot, a composition, and a
frame's activation with its decision memo
([where facts are decided](../../src/build/README.md#where-facts-are-decided)).
Each rebuilds part of the same structure: a settlement's parent link and a
closure's stamp repeat the order bind sites bind in, the trace's slot maps
are the edges a name is passed along, and a witnessed layer beside its
admission is a set of answers reused under the bindings that keep its
questions. The trace follows a name only where a call passes it on bare, and
names no binding of a body with more than eight
([the trace](../../src/build/README.md#the-trace)), so under

```text
EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)
EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
PRINT (WRAP 5)
```

`KEEP`'s facts under `Elem := LIST OF Number` wait for its frame, though the
text fixes them. A binding's refusal is raised at the resumption point for
every body beneath it, whichever bind site its inputs arrive at
([refusal](../../src/build/README.md#refusal)).

**Acceptance criteria.**

- The load builds one binding graph per base, through
  [the crate](binding-graph-crate.md), with koan's static types and verdicts
  as the fact domain and the one walk answering each `SiteQuestion`.
- The keys, the trace and the lookup of answers already made read the stored
  graph; no fold derives a bind site from a frontier, and no decision memo
  keys by a spine digest.
- Answers made under a binding are a `BoundInstance`, found by a `FOR ALL`
  name's solution, the type a type value denotes or a module's content,
  whether the load or a run cell made it.
- `KEEP`'s `BoundInstance` in the program above is made at the load, through
  the `PassesName` edge carrying `LIST OF Item`.
- No cap limits the instances the load makes for a body; a program whose
  instances the load cannot bound is refused
  ([refusal](../../design/binding-graph.md#refusal)).
- A name declared `Any` asks no `NameQuestion`, and a `SiteQuestion` waiting
  on one is answered by the run cell that reaches it.
- A refusal is raised at the bind site that owns its question; a test binds
  a body's entry and a later statement and observes each refusal at its own.
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *Where a `BoundInstance` rests — open.* This is the choice
  [one build](README.md#open-choices) lists as where an untraced decision
  lives, now over one kind of thing. Program storage for an instance the
  load makes; for one a run cell makes, the callable that binds it, an
  insert-only layer in the agent tier, or the frame's region.
- *Compositions — open.* A composition is a base with a graph of its own,
  built when composed; or it stays outside the graph and answers per `EVAL`
  frame.
- *Order against settlements retired — open.*
  [Settlements retired](settlements-retired.md) first, so the graph meets a
  depot of compositions alone; or this item first, so that item is the
  removal of a home nothing reads. Recommended: this item first.

## Dependencies

**Requires:**

- [The binding graph crate](binding-graph-crate.md) — the graph the build
  stores.

**Unblocks:**

- [The run reads site answers](run-reads-site-answers.md) — the answers a
  run performs are the graph's.
- [A small union split where it is bound](small-unions-split.md) — a split
  is an `AnswerBranch` at a bind site.
- [A recursion that grows its type is refused](../types/growing-recursion-refused.md)
  — the refusal reads a `PassesName` cycle.
- [The build image](build-image.md) — the image writes the graph.

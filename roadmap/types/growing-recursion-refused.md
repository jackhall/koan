# A recursion that grows its type is refused

**Problem.** A recursive body may call itself at a type built over its own
`FOR ALL` name:

```text
EXPR FOR ALL #[Item] #(DEEPEN y :Item) -> Any = #(DEEPEN [y])
```

Each call binds `Item` one level deeper — `Number`, `LIST OF Number`,
`LIST OF (LIST OF Number)` — so no binding of the body is its last. The load
admits the program, and the run decides the body afresh under each new type,
at every call ([activations](../../src/build/README.md#activations)); every
fact downstream of one waits for the run with it.

**Acceptance criteria.**

- A closed walk of laid calls whose `PassesName` mappings, composed around
  it, give a `FOR ALL` name a type built over itself is refused at build
  ([refusal](../../design/binding-graph.md#refusal)). The walk counts every
  candidate a call could select, and the message names a call on it and the
  `FOR ALL` name and asks for a declared recursive type.
- `DEEPEN` above is refused, and so is
  [the design's `MIX`](../../design/binding-graph.md#refusal), whose calls
  each repeat alone.
- A cycle that passes each name on as itself, renames names, or passes fixed
  content somewhere around it builds as before; under

  ```text
  EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)
  EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
  ```

  `WRAP` builds, since no walk leads back from `KEEP` to `WRAP`.
- The same recursion written over a declared recursive type builds, and one
  `BoundInstance` serves every depth.
- A cycle reached through a callable the text cannot follow — one read as a
  value, one an `EVAL` runs — is not refused.

**Directions.**

- *Which mappings count as growing — decided.* The composition around a
  whole closed walk, never one call alone. A name is built over itself only
  where every call on the walk hands it on, so a walk that passes fixed
  content at one call repeats and builds.
- *Inferring the recursive type — deferred.* A union the build infers in the
  declaration's place waits on anonymous recursive types
  ([types](README.md#unplanned-work)).

## Dependencies

The refusal is one case of the rule that the build refuses what it cannot
bound ([the binding graph](../../design/binding-graph.md#refusal)).

**Requires:**

- [The build over the binding graph](../one-build/build-over-the-binding-graph.md)
  — the refusal reads a `PassesName` cycle.

**Unblocks:** none — a leaf.

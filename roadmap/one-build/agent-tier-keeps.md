# What the agent tier keeps

The tier between the frozen load and a region, decided once nothing settles
into it.

**Problem.** The agent tier exists for settlements under the root: a
settlement rests the types its facts name, the binding's nodes relaid from
its image and the verdicts it reached there, so they outlive its eviction and
every frame under the binding reads them
([the agent tier](../../design/build-storage.md#the-agent-tier),
[the agent tier as the warm tier](../../design/type-tiers.md#the-agent-tier-as-the-warm-tier)).
It also takes what no settlement lays: a run's verdict over two load types
the load never asked, which no run may write into the frozen table, and the
symbol overlay of the names an agent's `EVAL`s parse. The load rests a
traced binding's types in the frozen load
([where facts are decided](../../src/build/README.md#where-facts-are-decided)),
so once [the build over the binding graph](build-over-the-binding-graph.md)
removes settlements, nothing settles into the tier, and what it still holds
— the overflow verdicts, the overlay, and whatever type a frame writes load
content into that its branch would otherwise mint per hop — is unmeasured.

**Acceptance criteria.**

- A list of what the tier holds after that item, measured over the example
  programs: types, verdicts and symbols, by what laid each.
- A ruling, recorded in [build storage](../../design/build-storage.md), on
  whether the tier stays — as the warm tier for the types frames close, as a
  verdict overflow, or as the overlay alone — or folds into the frozen load
  and the region branch, with the tiers' order
  ([type tiers](../../design/type-tiers.md#the-tiers)) rewritten to it.
- [The build image](build-image.md) knows what to write: whatever the tier
  keeps that is load content is in the image's cold layer, and the rest in
  its warm layer or not at all.

**Directions.**

- *The warm tier for a type a frame closes — open.* A type with load
  content written in is still finite per text, so it may live in an
  insert-only tier as a settlement's types do today; or it rests in the
  frame's branch and a loop mints it per hop.
- *The overlay — open.* The symbol overlay is per agent whatever the tier's
  fate; whether it stays beside a type tier or becomes a structure of the
  agent's own.

## Dependencies

**Requires:**

- [The build over the binding graph](build-over-the-binding-graph.md) —
  nothing settles into the tier before it is measured.

**Unblocks:**

- [The build image](build-image.md) — which of the tier's content is the
  cold layer's and which the warm's.

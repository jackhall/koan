# What the agent tier keeps

The tier between the frozen load and a region: what it holds measured, and
whether it keeps the types frames close ruled.

**Problem.** The agent tier holds the verdicts a run reaches over load or
agent types, which no run may write into the frozen table, beside the symbol
overlay of the names an agent's `EVAL`s parse; no run lays a type into it
([the agent tier](../../design/build-storage.md#the-agent-tier),
[the tiers](../../design/type-tiers.md#the-tiers)). How much of each a
program lays there is unmeasured, and so is whether a tier of types earns
its place between the frozen load and a region for verdicts alone.

No tier keeps a type a frame closes across frames
([the warm tier](../../design/type-tiers.md#the-warm-tier)). A frame keeps
what its binding closes a type to only where the type, the binding's content
and the closed type all rest in the frozen load (`rebound_under` in
[`graph.rs`](../../src/dispatch/graph.rs)). Any other type a frame closes is
minted into its scratch, which a frame keeps warm across its own statements
alone
([the scratch branch as a cache](../../design/type-tiers.md#the-scratch-branch-as-a-cache)).
So a loop calling one callable under one binding closes such a type again
each hop, under the program and under a composition alike; a composition's
facts name types the frozen load does not hold more often, since its depot
slot holds no type
([instances and compositions](../../design/build-storage.md#instances-and-compositions)).

**Acceptance criteria.**

- A list of what the tier holds, measured over the example programs:
  verdicts and symbols, by what laid each.
- The measurement counts, for a loop calling one composed callable under
  one binding and for the same loop in written code, the types a frame
  closes per hop.
- A ruling, recorded in [build storage](../../design/build-storage.md), on
  whether the tier stays — as the warm tier for the types frames close, as a
  verdict overflow, or as the overlay alone — or folds into the frozen load
  and the region branch, with the tiers' order
  ([type tiers](../../design/type-tiers.md#the-tiers)) rewritten to it.
- Where the ruling keeps a type a frame closes warm, a loop calling one
  callable under one binding closes each such type once, not once per hop,
  and what it keeps warm lies under a budget.
- No depot slot holds a type.
- [The build image](build-image.md) knows what to write: whatever the tier
  keeps that is load content is in the image's cold layer, and the rest in
  its warm layer or not at all.

**Directions.**

- *The warm tier for a type a frame closes — open.* A type with load
  content written in is still finite per text, so it may live in an
  insert-only tier of the agent's; or in a memo in the frame's activation
  keyed on its binding, which a hop's hand-off carries; or the load's facts
  name the closed term once, so a frame mints it from the term in O(1); or
  it rests in the frame's branch and a loop mints it per hop.
- *The overlay — open.* The symbol overlay is per agent whatever the tier's
  fate; whether it stays beside a type tier or becomes a structure of the
  agent's own.

## Dependencies

**Requires:** none.

**Unblocks:**

- [The build image](build-image.md) — which of the tier's content is the
  cold layer's and which the warm's.

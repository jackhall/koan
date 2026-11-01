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
symbol overlay of the names an agent's `EVAL`s parse. Once
[traced layers at load](traced-layers-at-load.md) rests a traced binding's
types in the frozen load and [settlements retired](settlements-retired.md)
decides the rest in activations, nothing settles into the tier, and what it
still holds — the overflow verdicts, the overlay, and whatever an untraced
binding's decision rests that a frame's branch would otherwise relay per hop
— is unmeasured.

**Acceptance criteria.**

- A list of what the tier holds after the two items above, measured over the
  example programs: types, verdicts and symbols, by what laid each.
- A ruling, recorded in [build storage](../../design/build-storage.md), on
  whether the tier stays — as the warm tier for untraced decisions, as a
  verdict overflow, or as the overlay alone — or folds into the frozen load
  and the region branch, with the tiers' order
  ([type tiers](../../design/type-tiers.md#the-tiers)) rewritten to it.
- [The build image](build-image.md) knows what to write: whatever the tier
  keeps that is load content is in the image's cold layer, and the rest in
  its warm layer or not at all.

**Directions.**

- *The warm tier for an untraced decision — open.* An untraced binding of
  load content is still finite per text, so what its decision rests may live
  in an insert-only tier as a settlement's does today; or it rests in the
  frame's branch and a loop relays it per hop, bounded by the decision memo.
- *The overlay — open.* The symbol overlay is per agent whatever the tier's
  fate; whether it stays beside a type tier or becomes a structure of the
  agent's own.

## Dependencies

**Requires:**

- [Settlements retired](settlements-retired.md) — nothing settles into the
  tier before it is measured.

**Unblocks:**

- [The build image](build-image.md) — which of the tier's content is the
  cold layer's and which the warm's.

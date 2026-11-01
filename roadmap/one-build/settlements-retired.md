# Settlements retired

A binding the load cannot trace decided in the frame's activation, and the
arena left holding compositions alone.

**Problem.** Once [traced layers at load](traced-layers-at-load.md) lands, a
settlement serves only a binding of load content the trace cannot follow — a
callee reached through a value the load cannot read, a call past a header or
a barrier. For that remainder the arena keeps its whole apparatus: a key, a
parent link, a borrower count, a generation, a pin on a composition, eviction
to a budget, the binding's image relayed into the agent tier, and a stamp
every closure carries to replay its chain after an eviction
([the build arena](../../src/scheduler/README.md#the-build-arena),
[references between frames and the arena](../../src/build/README.md#references-between-frames-and-the-arena)).
A binding of run content already decides without any of it, in the frame's
activation.

**Acceptance criteria.**

- A body under a binding the trace cannot name decides in its activation, as
  a body under run content does.
- The arena holds compositions alone: no settlement key, parent link, pin,
  held count or stamp replay, and no `Seek` but a composition's. A closure
  carries no settlement handle and no `Stamp`.
- A loop spawning such a body each hop decides it once, not per hop.
- No settlement lays into the agent tier; what the tier then holds is
  [its own item](agent-tier-keeps.md).
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *Where an untraced decision is kept — open.* A frame's decision memo rests
  in the region its result is built in and a spawned frame's is extended
  copy-on-write from its asker's, so a decision a spawned frame makes dies
  with its home ([activations](../../src/build/README.md#activations)): a loop
  spawning an untraced keyed body each hop would decide it each hop. Either
  decisions are written back into the asker's memo, which gives up the memo's
  immutability; or the callable value carries the decisions made under it, as
  it carries its captures, so a call finds them on the callee; or bindings of
  load content get one memo at the run root keyed by digest — the arena's
  table without its tree, a run-rooted registry of the kind koan keeps off
  the runtime. The third acceptance criterion decides.
- *The budget — open.* With compositions the arena's only tenants, whether
  [the budget](../features/bound-settings.md) still bounds bytes or
  counts compositions.

## Dependencies

**Requires:**

- [One layer, wherever laid](one-layer.md) — a decision moves to the
  activation in the form it already has.
- [Traced layers at load](traced-layers-at-load.md) — what the load lays
  leaves the arena only the untraced remainder.

**Unblocks:**

- [What the agent tier keeps](agent-tier-keeps.md) — with nothing settling
  into it, the tier's content is measured.
- [The build image](build-image.md) — the arena it writes holds compositions
  alone, and program storage every layer the load laid.

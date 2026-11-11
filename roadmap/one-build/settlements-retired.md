# Settlements retired

A binding the load cannot trace decided in the frame's activation, and the
depot left holding compositions alone.

**Problem.** A settlement serves only a layer the load did not lay
([where facts are decided](../../src/build/README.md#where-facts-are-decided)): a binding of load
content entering at a site the trace cannot follow — a callee reached
through a value the load cannot read, a call past a header or a barrier — a
binding of a body the trace leaves open, and each layer a site beneath
either names. For that remainder the depot keeps its whole apparatus: a key
computed from the values at each seek, a parent link, a borrower count, a
generation, a pin on the composition whose tree the binding names, eviction
to a budget, the binding's image relayed into the agent tier, and a stamp
every closure carries to replay its chain after an eviction
([the build depot](../../src/scheduler/README.md#the-build-depot),
[references between frames and the depot](../../src/build/README.md#references-between-frames-and-the-depot)).
A binding of run content already decides without any of it, in the frame's
activation. A call by name keyed on a `FOR ALL` name solves its group twice:
`seek` ([`settle.rs`](../../src/program/settle.rs)) runs `admitted` over the
arguments for the key, and the frame's birth in
[`body.rs`](../../src/program/body.rs) runs it again, since the birth carries
no solution.

**Acceptance criteria.**

- A body under a binding the trace cannot name decides in its activation, as
  a body under run content does.
- The depot holds compositions alone: no settlement key, parent link, pin,
  held count or stamp replay, and no `Seek` but a composition's. A closure
  carries no `Stamp`, and its `DepotRef` names the root, a layer the load
  laid or a composition.
- A loop spawning such a body each hop decides it once, not per hop.
- A call by name keyed on a `FOR ALL` name solves its group once, in its
  frame: a test counts `admitted` calls over one and counts one.
- No walk at run lays into the agent tier; what the tier then holds is
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
  load content get one memo at the run root keyed by digest — the depot's
  table without its tree, a run-rooted registry of the kind koan keeps off
  the runtime. The third acceptance criterion decides.
- *The budget — open.* With compositions the depot's only tenants, whether
  [the budget](../features/bound-settings.md) still bounds bytes or counts
  compositions.

## Dependencies

**Requires:** none.

**Unblocks:**

- [What the agent tier keeps](agent-tier-keeps.md) — with nothing settling
  into it, the tier's content is measured.
- [The build image](build-image.md) — the depot it writes holds compositions
  alone, and program storage every layer the load laid.

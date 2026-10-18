# Specialization keys over frontiers

A specialization's key names every binding its seam can settle.

**Problem.** A sealed body's specialization keys on its segment and the
parameters its invariant sites read
([keys](../../src/build/README.md#keys-which-bindings-make-a-specialization);
`Caching::invariant`, built in `src/dispatch/statics/caching.rs`). The load
records each fact's frontier — the unknowns it waits on, by segment and
address (`Frontier` in `src/scope/shape/facts.rs`) — and the seams that
complete one ([frontiers and seams](../../src/build/README.md#frontiers-and-seams)),
but no frontier name reaches the key: a specialization is one body under one
binding of its invariant-site parameters alone, and a fact waiting on any
other name has no key to be settled under.

**Acceptance criteria.**

- A specialization keys on its segment and the content at the names any
  frontier at its entry waits on, facts in nested bodies included, beside the
  parameters its invariant sites read.
- A parameter keys only where its declared type is closed under `Module` or it
  holds a type value (`Pass::keys` in `src/dispatch/statics/caching.rs`), and
  a name a hop of the body's own tail rebinds is not keyed.
- A keyed frame seeks whatever its placement.
- Two calls of one body whose arguments differ only at a frontier name build
  two specializations; two whose arguments agree at every keyed name share
  one.
- An evicted specialization costs a recomputation and never a different
  answer.

**Directions.**

- *Where the frontier names come from* — decided. The load's record of each
  fact's frontier, read at the entry seam of the body a frame runs and through
  its nested bodies' facts; the key adds no name the load does not list. A
  frontier name whose parameter neither is closed under `Module` nor holds a
  type value keys nothing, and the facts waiting on it stay unsettled per
  [activation typing](activation-typing.md).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — a fact is settled only under a
  key that covers its frontier.

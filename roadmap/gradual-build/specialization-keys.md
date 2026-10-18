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

- A callable's specialization, whether or not its body caches, keys on its
  segment and the content at each of its own entry-bound names that an entry
  seam of its body, or of a callable nested in it, waits on, beside the
  parameters its invariant sites read.
- A parameter keys only where its declared type is closed under `Module` or it
  holds a type value (`Pass::keys` in `src/dispatch/statics/caching.rs`), and
  a name a hop of the body's own tail rebinds, passing anything but that same
  parameter at it, is not keyed.
- A `FOR ALL` name's content is the call's solution at it, digested as the type
  value the frame binds: carried by a keyworded call, held by an instance, and
  solved by the asker for a call by name. A call past a header or a barrier,
  of a body keyed on a `FOR ALL` name, is born cold.
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
- *Whose names a nested body's frontier keys* — decided. Every entry seam in
  the body's subtree counts, so `outer` keys its `m` where a fact of a nested
  callable waits on `m` and the nested callable's own `Elt`, and the nested
  specialization sits under a chain whose keys cover the frontier. A frontier
  whose seam is a unit, or a quote's code entry, keys nothing.
- *A hop of a body whose name escapes* — decided. It seeks with its actual
  content, as a call of another callee does. Only a body whose name does not
  escape has every hop into itself recorded as a tail edge, so only its hop
  borrows its predecessor's specialization with no lookup: an escaping body
  can tail-call itself through a value the load cannot follow, rebinding a
  keyed name it never sees rebound.
- *A `FOR ALL` name's solution past a header or a barrier* — deferred to
  [gradual-build's unplanned work](README.md#unplanned-work). The frame solves
  the node's own group only after retyping or crossing the arguments, and the
  crossing is a value operation, so the asker cannot compute it.
- *An `EVAL` frame over a written quote's code* — deferred to
  [gradual-build's unplanned work](README.md#unplanned-work). An `EVAL` frame
  seeks nothing here, and a seam at a code entry keys nothing.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Activation typing](activation-typing.md) — a fact is settled only under a
  key that covers its frontier.

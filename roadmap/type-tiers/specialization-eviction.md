# Evicting specializations

A policy under which a build cell evicts the specializations it holds.

**Problem.** A [specialization](../../src/build/README.md#two-trees-under-one-drain)
lives from the first miss on its binding to the program's end: the drain's
eviction door, `Scheduler::evict` in `src/scheduler/drain.rs`, is test-only,
and nothing calls it in a run. A key digests the content of the arguments a
caching frame's invariant sites read (`src/program/specialize.rs`), so a loop
that calls such a frame over new content each iteration builds one
specialization per iteration, each a tree cell with a region and a type
branch, all held until the program ends: such a program's memory grows with
its iteration count, where a loop's bytes and lattice otherwise hold flat. A
helper with no tail loop is the sharpest case: every one of its parameters is
trivially invariant, so each call over new content is a miss. No
test runs such a loop; `a_birth_keyed_on_a_list_digests_it_at_a_cost_below_its_length`
in `src/dispatch/tests/caching.rs` calls over one list only. A frame born with no specialization
warms the scratch its asker's park holds, which resets after the delivery
([the scratch branch as a cache](../../design/type-tiers.md#the-scratch-branch-as-a-cache));
a keyed frame's warm tier never resets. Each such birth also walks its keyed
arguments to digest them. A frame under an evicted ancestor is born cold and
nothing rebuilds the ancestor, and no test evicts a specialization while a
frame still borrows it.

**Acceptance criteria.**

- A loop calling a caching frame over a new invariant binding each iteration
  holds a number of live specializations, and bytes in them, independent of
  its iteration count.
- A build cell evicts only a childless specialization, and an eviction costs
  a rebuild on the next miss and never a different answer.
- A test evicts a specialization while a frame borrows it: the frame runs to
  its end over the dead but undisposed specialization, the next miss on its
  key builds a fresh one, and the old one disposes with its last borrower.
- A frame whose specialization's ancestor was evicted runs to the same
  answer as one born warm.

**Directions.**

- *What a parent counts as warmth* — open. The parent prices warmth against
  the child's bytes; cellgraph ships the bytes and no threshold, as the spare
  list's bound against recent demand does. Hits since the last eviction, the
  binding's recency, and the bytes a rebuild would lay are the candidates.
- *Whether a borrower count enters the price* — open. A region lend keeps its
  lender's bytes alive while any borrower stands, and the verdict sees no
  price for that retention.
- *A stale ancestor* — open. A frame under an evicted ancestor is born cold.
  Rebuilding the chain needs each callable to keep the digests of the
  bindings above it; leaving it cold costs warmth only.
- *Which bindings key at all* — open. A key over run-built content, a list's
  digest, may never be hit twice; demand observed at the call site could
  decide that such a binding makes no specialization, as varying content
  makes none.
- *Sibling redundancy* — open. An inner body's facts whose frontier names
  only the inner rigid are re-solved once per outer binding. A hoist to a
  child of the root would share them at the cost of a second parent; left as
  cache the eviction policy prices until a program shows the cost.

## Dependencies

It prices the specializations [the build module](../../src/build/README.md)
holds, through cellgraph's
[named lend and release](../../cellgraph/src/tree/README.md#death).

**Requires:** none — the build tree's eviction mechanism stands.

**Unblocks:** none.

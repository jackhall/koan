# Evicting specializations

A build cell's specializations as a small least-recently-born cache.

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
trivially invariant, so each call over new content is a miss:
`a_fresh_loop_calling_a_shares_caching_helper_holds_each_specialization_flat`
in `src/dispatch/tests/caching.rs` asserts one specialization per link of
the list its loop walks. A parent's table (`SpecTable` in
`src/values/specialization.rs`) records a key and a handle per child and
nothing about its use, so nothing a parent holds says which child a miss
should displace; and it is insert-only, so the same loop adds an entry to
the parent's table per iteration.

**Acceptance criteria.**

- A loop calling a caching frame over a new invariant binding each iteration
  holds a number of live specializations, and bytes in them and in their
  parents' tables, independent of its iteration count, including where the
  frame's body calls a keyed callable of its own.
- A build cell's table is a fixed run of entries, laid with the cell and
  never grown: the build root's as long as the root bound, a
  specialization's as long as the nested bound. Both bounds are parameters
  of the load, 16 and 4 by default, each at least one.
- Eviction runs at a miss in `Scheduler::seek`, before the miss creates its
  child, and nowhere else: a parent with no free entry releases its least
  recently born child with its subtree, leaf first, and the new child takes
  that child's entry.
- Each `SpecEntry` carries the parent's birth count at its specialization's
  last birth, and nothing else is added to the entry. A birth is the miss
  that creates the specialization, a seek that finds it live, or a hop of the
  same callee under it: `wrapped_peeks` in `src/dispatch/tests/caching.rs`,
  under a root bound of two, runs every `WRAP` hop warm.
- An eviction costs a rebuild on the next miss and never a different answer.
- A borrower's count does not enter the choice: a test evicts a
  specialization while a frame borrows it, and the frame runs to its end over
  the dead but undisposed specialization, the next miss on its key builds a
  fresh one, and the old one disposes with its last borrower. Where that
  borrower is a `Shares` frame, the test observes that the old specialization
  is still undisposed after the frame's release and disposes only when the
  frame's host's scratch next resets.
- A frame whose specialization's ancestor was evicted is born cold and runs
  to the same answer as one born warm.

**Directions.**

- *The bound* — decided. A fixed count of live children per parent, set at
  load: the build root's table is shared by every top-level keyed callee, so
  it takes a wider bound than a specialization's, whose bound is a branching
  factor that nesting multiplies.
- *Which child goes* — decided. The least recently born child, with or
  without children: a policy that evicts only childless children starves
  once every child of a parent has a child of its own, as a frame keyed per
  iteration whose body calls a keyed callable leaves them.
- *A setting a user can name* — deferred to
  [setting the specialization bounds](bound-settings.md).

## Dependencies

It prices the specializations [the build module](../../src/build/README.md)
holds, through cellgraph's
[named lend and release](../../cellgraph/src/tree/README.md#death). Which
bindings make a key at all is [digest demand](digest-demand.md)'s.

**Requires:** none — the build tree's eviction mechanism stands.

**Unblocks:**

- [Setting the specialization bounds](bound-settings.md) — the bounds it
  makes load parameters.

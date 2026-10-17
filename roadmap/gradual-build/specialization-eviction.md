# Evicting specializations

A build cell's specializations as a small least-recently-hit cache.

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
should displace.

**Acceptance criteria.**

- A loop calling a caching frame over a new invariant binding each iteration
  holds a number of live specializations, and bytes in them, independent of
  its iteration count.
- Eviction runs at a miss in `Scheduler::seek`, before the miss creates its
  child, and nowhere else: a parent past its bound releases its least
  recently hit childless child. Each `SpecEntry` carries the parent's miss
  count at the entry's last hit, and nothing else is added to the entry.
- A build cell evicts only a childless specialization, and an eviction costs
  a rebuild on the next miss and never a different answer.
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

- *The bound* — open. A fixed count of live children per parent, a constant
  of the scheduler; or a byte bound over the children's regions, through a
  shipped `region_bytes` door on cellgraph. Recommended: a fixed count, since
  the specializations a loop needs warm at once are few and a count needs no
  new door.

## Dependencies

It prices the specializations [the build module](../../src/build/README.md)
holds, through cellgraph's
[named lend and release](../../cellgraph/src/tree/README.md#death). Which
bindings make a key at all is [digest demand](digest-demand.md)'s.

**Requires:** none — the build tree's eviction mechanism stands.

**Unblocks:** none.

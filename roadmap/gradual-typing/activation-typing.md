# Activation typing

The type pass re-entered per activation, where an argument's content is known.

**Problem.** The load types a body once, before any argument exists, and a
type it cannot fix there is left unknown and elaborated by the run where each
read happens
([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)).
A body taking a module parameter reads `p.Carrier` as a leaf at load
([comparing two paths](../../design/modules.md#comparing-two-paths)), so every
call inside it over that carrier is *maybe*, admitted and ranked at each use,
though at the frame's entry the argument's content closes the tree and every
such verdict could be drawn once for the whole body. Nothing re-enters the pass
between the load and a read.

**Acceptance criteria.**

- A body whose parameter's content is known at frame entry has its static
  types and verdicts recomputed over that content, and a call the recomputed
  pass judges *always* runs without admitting or ranking.
- The recomputation is memoized by the shape and the argument's content, the
  memo is lossy, and a forgotten entry costs a recomputation and never a wrong
  answer.
- Every type the recomputation interns, and every verdict over one, lives in
  the activation's branch and is released with the frame.
- A verdict the recomputed pass reaches holds at every run of that activation,
  by the same laws the load's verdicts hold by.

**Directions.**

- *What triggers re-entry — open.* Every frame whose shape names a leaf the
  argument closes; or only a frame the load marks as benefiting, by a count of
  *maybe* verdicts over that leaf.
- *What re-entry may refuse — open.* Whether a verdict of *never* at frame
  entry faults the call there, as the load refuses a use left with no
  candidate, or is left to fault at the use.

## Dependencies

**Requires:**

- [Type branches](../type-tiers/type-branches.md) — the recomputation's types and
  verdicts die with the frame.

**Unblocks:** none — a leaf.

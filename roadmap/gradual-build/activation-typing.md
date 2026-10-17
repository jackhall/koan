# Activation typing

A frame's facts settled at its seams, over the content its bindings bring.

**Problem.** A path through a parameter is a path variable of a class
([paths](../../src/dispatch/README.md#paths)): two paths of one class are one
type, and a call solving a variable from both is *always*. A candidate that
meets the path against a closed type, an overload at `Number` or a slot
`:Number`, or against a second class, is *maybe*, admitted and ranked at each
use, though at the frame's entry the argument's content closes the class and
every such verdict could be drawn once for the whole body. The load's pass is
not re-entered between the load and a read
([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)),
and the run binds a path variable per read (`elaborate::bound_path`).

**Acceptance criteria.**

- A frame whose binding completes a frontier at entry has every fact in that
  frontier settled in the specialization its seam and binding name before
  its first value step, and a call judged *always* there runs without
  admitting or ranking.
- A second frame under the same binding, whatever its placement, reads that
  specialization and recomputes nothing.
- A frontier with a member a statement binds settles at that statement in
  the run cell's scratch.
- An evicted specialization costs a recomputation and never a different
  answer.
- Every type a specialization interns, and every verdict over one, lives in
  the specialization's branch and goes with its eviction and its last
  borrower's death.
- A verdict a specialization reaches holds at every run of a frame under
  that binding, by the laws the load's verdicts hold by.

**Directions.**

- *What triggers re-entry* — decided. The seams the load lists: a frame
  resumes at entry when a rigid's binding completes a frontier, and at a
  statement when a binding there completes one. Nothing counts *maybe*
  verdicts to decide.
- *What re-entry may refuse* — decided. A use left with no candidate at a
  resumption faults at the resumption point, as the load refuses a use left
  with no candidate.

## Dependencies

**Requires:**

- [Monotone elaboration](monotone-elaboration.md) — the solver that parks and
  resumes.

**Unblocks:** none — a leaf.

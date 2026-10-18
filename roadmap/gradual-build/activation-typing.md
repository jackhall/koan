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
- A specialization's one step runs its resumption to quiescence and
  quiesces, over a chain through its own branch. Facts the resumption
  settles land in that cell, and what it leaves parked is the cell's
  residual.
- A specialization keys on its segment and the content at the names any
  frontier at its entry waits on, facts in nested bodies included, beside the
  parameters its invariant sites read. A parameter keys only where its
  declared type is closed under `Module` or it holds a type value
  (`Pass::keys` in `src/dispatch/statics/caching.rs`), and a name a hop of
  the body's own tail rebinds is not keyed. A keyed frame seeks whatever its
  placement.
- A second frame under the same binding, whatever its placement, reads that
  specialization and recomputes nothing.
- A frontier with a member a statement binds settles at that statement in
  the run cell's scratch, and no run cell's lend changes after its creation.
- A use left with no candidate at a resumption faults the asker at the
  resumption point, naming the call.
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
- *What else a resumption refuses* — open. A resumption may reach any refusal
  the load can, an ambiguity or a return that can never be satisfied among
  them; whether each faults at the resumption point too, or only a use with
  no candidate, is unruled.
- *Deepening the borrow at a mid-body seam* — decided. Never. A statement's
  binding is either determined by entry content — a `MATCH` arm's `it` over
  a parameter, a `USING … SCOPE` key over one, a path hop rooted at one —
  and settled at the entry seam, since a load with that content declared
  would type it closed; or produced by the run — an `EVAL`'s or an action's
  result — which is varying content and keys nothing, so the run cell
  resumes it into its own scratch. Cellgraph's lend stays fixed at creation.
  A `Shares` frame reaches its specialization through
  [a lend of its own](../../design/type-tiers.md#the-specialization-as-the-warm-tier). A child specialization
  whose facts name a type its parent minted relays it into its own branch,
  insert-if-absent, so no branch links its parent's and a run cell's chain
  probes one specialization.
- *A specialization that resumes* — decided, on the working assumption that
  no binding wakes a quiesced build cell: a specialization is one seam under
  one binding, what its resumption leaves parked waits on a binding its key
  does not fix, and two frames under it may fix that binding differently,
  so the parked remainder is the cell's residual and the work over it runs
  in the cell the next binding names — a child specialization, which reads
  the residual through the parent region it borrows, or a run cell's
  scratch. `Step::enlist` grows no successor. A case where the same cell is
  the right place to resume — a remaining unknown with one possible binding
  — would reopen this; none is known. A capture is
  answered by keying on the seam's names: `inner`'s fact on a capture `xs`
  waits at `outer`'s entry, so `outer` keys on `xs` where `xs` is a
  parameter that keys, and a fact whose frontier the chain's keys do not
  cover resumes cold. A body whose name
  escapes keys on its names like any other, since a hop into another callee
  seeks with the actual content and only a hop of the same callee reuses a
  specialization with no lookup; its mint sites stay varying.

## Dependencies

**Requires:** none — its prerequisites have shipped.

**Unblocks:** none — a leaf.

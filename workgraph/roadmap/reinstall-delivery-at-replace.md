# Delivery at replace for reinstallation

**Problem.** `DepRow.handoff` parks the displaced incarnation's anchor so the
retiring region outlives the reinstalled incarnation's *first step*, where the
framed tail invoke reads the call's working expression — its parts run, its
bumped structural cache, and its resting argument cells — out of that region;
the run loop enforces the ordering with a take-before-step / drop-after-step
protocol ([dep_graph.rs](../src/scheduler/dep_graph.rs),
[run_loop.rs](../../src/machine/execute/run_loop.rs)). Nothing forces that read
to happen after the hop: the step that *emits* the replace still holds the
displaced anchor, and the argument bind already deep-clones into the new
incarnation's region under the argument's own reach, so the row-level hold pins
the retiring region longer than anything needs
([reach.md § Retention model](../design/reach.md#retention-model)).

**Acceptance criteria.**

- The framed tail invoke's argument bind runs in the step that emits the
  replace, against the freshly minted cart, so the reinstalled incarnation's
  first step reads nothing in the retiring region — no argument cell and no
  working expression. The retiring-anchor ordering is a local variable held
  across the install call.
- `DepRow.handoff` and the run loop's take-before-step / drop-after-step
  protocol are deleted.
- Copy verdicts free the retiring region at the replace; pin verdicts transfer
  it by hold into the new incarnation's anchor bundle.
- Workgraph Miri slate timeline: reinstall with loop-carried arguments, under
  both verdicts.

## Dependencies

**Requires:**


**Unblocks:** none — leaf.

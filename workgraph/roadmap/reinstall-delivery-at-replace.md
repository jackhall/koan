# Delivery at replace for reinstallation

**Problem.** `DepRow.handoff` parks the displaced incarnation's anchor so the
retiring region outlives the reinstalled incarnation's *first step*, where it
adopts the loop-carried arguments; the run loop enforces the ordering with a
take-before-step / drop-after-step protocol
([deps.rs](../src/scheduler/deps.rs),
[run_loop.rs](../../src/machine/execute/run_loop.rs)). With delivery at
finalize shipped, that adoption can happen inside the replace itself — the
outcome-apply path still has the displaced anchor in hand — so the row-level
hold pins the retiring region longer than anything needs
([reach.md § Retention model](../design/reach.md#retention-model)).

**Acceptance criteria.**

- Loop-carried arguments deliver at the replace: terminal sources deliver at
  wiring, construction operands merge at install, and the retiring-anchor
  ordering is a local variable held across the install call.
- `DepRow.handoff` and the run loop's take-before-step / drop-after-step
  protocol are deleted.
- Copy verdicts free the retiring region at the replace; pin verdicts transfer
  it by hold into the new incarnation's anchor bundle.
- Workgraph Miri slate timeline: reinstall with loop-carried arguments, under
  both verdicts.

## Dependencies

**Requires:**


**Unblocks:** none — leaf.

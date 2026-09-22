# EVAL dynamic dispatch

Evaluated code reaching registrations the static shape cannot see.

**Problem.** [Dispatch](dispatch.md) resolves a bucket key to a candidate list
when the shape is built, and an `EVAL` does the same when its own block shape is
built, at the `EVAL`'s written position. Two consequences follow. A registration
declared after the `EVAL`'s position, or by evaluated code, is never a candidate
for a keyworded use inside evaluated code. And a statement containing `EVAL` at
any depth waits on every unit declared before it, so a lambda whose `EVAL` would
only run when called still refuses the body with `EvalCycle` when a binder
before it waits on that statement.

**Acceptance criteria.**

- A keyworded use in evaluated code takes as candidates every registration
  visible at the `EVAL`'s position when the `EVAL` runs, including registrations
  evaluated code declared earlier in the same evaluation, under the same
  selection rule dispatch applies statically.
- An `EVAL` nested in a callable body on a binder's right-hand side no longer
  makes that binder's statement wait on every earlier unit; the read of an
  unbound name when the `EVAL` runs is a koan error value, not a scheduler panic.
- A candidate list a static site computed is unchanged by anything evaluated
  code does.

**Directions.**

- *What the run-time search reads — open.* The shape retains its defining scope
  wherever it contains an `EVAL`, so the search may walk activations rather than
  shapes; whether the extra candidates are collected by a second walk or the
  block shape is rebuilt per evaluation is undecided.

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — the static candidate list this relaxes.

**Unblocks:** none — leaf.

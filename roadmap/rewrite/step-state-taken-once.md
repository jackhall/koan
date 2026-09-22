# A step's state taken once, by type

**Problem.** A native step reads the state its cell holds and the scratch state
it parked through [`Step::state` and `Step::scratch`](../../src/scheduler/action.rs),
which take each out of an `Option` field of the `Step`. "A step takes its state
once" is therefore a run-time check: a second `state` panics, and a second
`scratch` quietly finds `None`. Every other rule of a step's life — that it ends
exactly once, that a park has a child to wait on, that it names no handle — is a
compile error. The state rides inside `Step` rather than beside it because a
`NativeStep` pointer higher-ranked over `'here` cannot take the bundle's
projection `<B::State as Reattachable<'graph>>::At<'here>` as a parameter
(rustc #100013), while the same projection compiles as a field of `Step`.

**Acceptance criteria.**

- A step that takes its state twice, or its scratch state twice, does not
  compile, pinned by a `compile_fail` doctest on each against a compiling
  control.
- `Step` carries no `Option` around its state or its scratch state and has no
  `expect` or `panic` on either path.
- A step that never takes its state still compiles, and its end still hands the
  scratch bump back when the scratch state was not parked again.
- The scheduler's tests and its Miri slate pass unchanged in what they assert.

**Directions.**

- *How the take becomes a type — open.* A by-value split: `Step::take(self)`
  hands back the state, the scratch state and a `Step` with neither, whose ends
  are the same, so the second take has nothing to call it on. Or a typestate
  parameter on `Step` (holding / taken) whose `state` method exists only while
  holding. Or pass the state beside the `Step` once rustc #100013 is fixed, if it
  is. *Recommended:* the by-value split — it adds no type parameter to every
  step's signature.

## Dependencies

**Requires:** none — the scheduler's `Step` ships.

**Unblocks:** none — a leaf.

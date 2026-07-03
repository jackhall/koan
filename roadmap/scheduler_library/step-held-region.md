# The step owns region liveness

Deliver guarantee 4 of
[design/scheduler-library.md](../../design/scheduler-library.md): during a
step, the machinery that runs the step holds the consumer's region owner, and
step code gets an **infallible** live-region handle — instead of every
consumer re-asserting liveness by hand.

**Problem.** The scheduler already keeps the consumer's frame cart alive
across a step (`NodeFrame.cart`, `src/scheduler/nodes.rs:55`), but that fact
is invisible to step code, so every consumer needing the destination frame
hand-rolls `scope.region_owner().upgrade().expect("… region owner is held for
the step")` — `dispatch/literal.rs:71-75` and :257-260,
`dispatch/constructors.rs:179-182`, `dispatch/single_poll.rs:182-187` and
:216-219, `runtime/submit.rs:99-102` — with three drifted expect wordings
("dispatching" / "consumer" / "classify scope's"). `scope_frame`
(`src/machine/core/kfunction/action.rs:39-44`) already names the invariant but
has one external caller (`builtins/catch.rs:79`). Two sites additionally
repeat the dest-brand construction
`KoanRegion::yoke_branded::<RegionRefFamily, _>(dest_frame, |b| b)`
(`constructors.rs:183`, `single_poll.rs:222`).

**Acceptance criteria.**

- Step-scoped contexts (`FinishCtx` and the dispatch-side context) expose a
  live-region/frame accessor **with no failure path** — no `Option`, no
  `expect` — derived from what the step machinery already holds.
- The seven sites above and `scope_frame`'s caller route through it;
  `region_owner().upgrade()` no longer appears in `dispatch/` or
  `execute/runtime/` production code (the `Scope`-internal uses at
  `scope.rs:879-881` are out of scope).
- The dest-brand construction (`yoke_branded::<RegionRefFamily, _>`) has one
  owner next to the accessor.
- Behavior unchanged; existing tests green.

**Directions.**

- *How the handle reaches step code — open.* (a) read the ambient active
  frame the run loop installs at `enter_slot_step` and expose it on the
  contexts; (b) thread the frame handle as an explicit context field filled
  at apply time. Recommended: (a) — the run loop already installs it; this
  item makes the existing hold visible instead of adding plumbing.
- *Non-step allocation untouched — decided.* `CallFrame` holding region
  handles for at-will allocation is the north star's other allocation mode;
  this item only fixes the step path.
- *Ambient bracket hygiene — deferred.* The save/restore weaknesses of
  `SlotStepGuard` (no Drop backing; the raw `active_in_contract_chain`
  writes) are a separate item; do not restructure the guard here.

## Dependencies

Touches the same finish bodies the
[`Await` envelope builder](await-envelope-builder.md) reshapes — lands
cleanest after it (soft ordering, not a prerequisite).

**Requires:** none — parallel migration track.

**Unblocks:** none tracked yet — the step construction context
(`ctx.alloc` / `alloc_with`) builds on this accessor.

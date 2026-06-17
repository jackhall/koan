# Node-lifetime lift and contract re-anchor

Thread distinct input/output node lifetimes through the lift and contract Done-boundary hooks so
their value re-anchor is node-to-node, retiring the `'run` fabrication the driver does to feed them.

**Problem.** The two Done-boundary workload hooks both move a value between per-call frames, but each
is typed at one collapsed `'run`, so the driver must fabricate `'run` to call them:

- [`NodeLift::lift`](../../src/machine/execute/lift.rs) is `lift(value: Carried<'run>, src:
  &Rc<CallArena>, dst: &'run RuntimeArena) -> Carried<'run>`, yet it allocates its output into `dst`
  (the consumer frame, a node-scale arena) and embeds an `Rc::clone(src)` anchor into the lifted
  object — so the forwarded `&` references the `KFunction` / `List` arms carry are kept alive by the
  embedded `Rc`, not by `'run`. The output genuinely lives at `dst`'s lifetime, not the run global.
- [`NodeFinalize::finalize_terminal`](../../src/machine/execute/finalize.rs) re-tags a coarsened
  terminal into `prev_function.home_arena()` — "the callee's captured-scope / arm call-site arena, a
  strict ancestor of the producer frame," again node-scale — and re-anchors the contract there.

Because both wear `'run`, the driver fabricates `'node -> 'run` to feed them:
[`read_lifted`](../../src/machine/execute/runtime.rs) reattaches the scheduler's `'node` read up to
`'run` before `lift`, and [`pin_carried_to_run`](../../src/machine/execute/outcome.rs) reattaches the
step terminal `'s -> 'run` in `apply_outcome` before the contract layer. Neither movement needs the
run global; the `'run` annotation is the only thing that makes them look like they do.

**Acceptance criteria.**

- `NodeLift::lift` and `NodeFinalize::finalize_terminal` carry distinct input/output lifetimes
  (`lift(value: Carried<'i>, src, dst: &'o RuntimeArena) -> Carried<'o>` and the contract dual), not
  one collapsed `'run`.
- The producer→consumer (lift) and producer→home-arena (contract) re-anchor is node-scale (`'i ->
  'o`), witnessed by the held producer-frame `Rc` plus the embedded anchor — not a `'node -> 'run`
  fabrication.
- `read_lifted` performs no `'run` reattach: the scheduler hands the lift hook a destination-lifetime
  value.
- `pin_carried_to_run` no longer reattaches `'s -> 'run` for the contract layer; `apply_outcome`
  feeds the contract hook a node-lifetime terminal.
- `'run` survives only for the genuine run-global root drain (the consumer-less terminal re-homed
  into the run arena), not as the currency of every dep-delivery and Done step.

**Directions.**

- *Thread `<'i, 'o>` through both hooks — decided.* Input value lifetime and output/destination
  arena lifetime are distinct; the re-anchor between them is the node-scale movement.
- *Where the re-anchor lives — open.* The scheduler holds both frames and drives both hooks, so it
  can own the `'i -> 'o` re-anchor (handing the hook a destination-lifetime value), or the hook can
  keep it behind the `NodeLift` / `NodeFinalize` boundary. Recommended: scheduler-owned, mirroring
  the `'node` read surface — it concentrates the value-movement re-anchor in one place.
- *The `KObject`-invariant copy and embedded `Rc` anchor stay a Koan hook detail — decided.* The
  arena→arena `KObject` copy and the escaping-closure anchor decision remain in `lift.rs`; the
  scheduler names no `KObject`, so only the lifetime re-anchor (not the copy) can move scheduler-side.

## Dependencies

On the same value-movement seam as the now-shipped scheduler-owned value erasure (the `'node` read
surface and `Erased<W::Value>` store this rethread extends to the lift / Done hooks); update
[design/memory-model.md § Arena lifetime erasure](../../design/memory-model.md#arena-lifetime-erasure)
and [design/per-call-arena-protocol.md](../../design/per-call-arena-protocol.md) if the lift / Done
re-anchor it describes changes.

**Requires:** none — its prerequisite (scheduler-owned value erasure) shipped.

**Unblocks:** none tracked yet.

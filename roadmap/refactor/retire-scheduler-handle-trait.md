# Retire `SchedulerHandle` as a trait

Now that `Scheduler` is the only `impl SchedulerHandle`, collapse the trait to inherent
methods or split it into two narrower capability traits.

**Problem.** [`SchedulerHandle`](../../src/machine/core/kfunction/scheduler_handle.rs) is a
trait with exactly one implementor — [`Scheduler`](../../src/machine/execute/scheduler.rs)
(`scheduler.rs:355`). The second impl (the old `DispatchCtx` forwarder) is gone now that
dispatch decides against a read-only [`DispatchCx`](../../src/machine/execute/dispatch/ctx.rs)
and returns a [`DispatchOutcome`](../../src/machine/execute/dispatch/outcome.rs) the harness
applies. A single-implementor trait buys no polymorphism: it is dynamic dispatch (`&mut dyn
SchedulerHandle` reaches builtin bodies) and an indirection where an inherent method would do.
The trait also mixes read methods (`current_scope`, `current_frame`, `is_result_ready`) with
write methods (`add_park_edge`, `add_dispatch_here`, `free`) on one surface, so a builtin body
that should only read still receives the full write-capable handle.

**Acceptance criteria.**

- `SchedulerHandle` is either gone (its methods inherent on `Scheduler`) or split into a
  read-only and a write-capable trait; no single-implementor trait survives solely to wrap
  `Scheduler`.
- A builtin body's handle exposes only the capabilities it uses — a read-only body cannot
  reach a write method through its type.
- `cargo test` and the Miri slate stay green; no behavioral change.

**Directions.**

- *Inherent methods vs narrow trait split — open.* Collapsing to inherent `Scheduler` methods
  is the smallest change but keeps `&mut dyn` builtin bodies needing a trait object;
  splitting into `SchedulerRead` / `SchedulerWrite` keeps the body-facing seam abstract while
  separating the two capabilities. Recommended: scope the builtin-body handle first, since it
  is the only caller that needs a trait object at all.

## Dependencies

Follow-up to the shipped dispatch write-effect contract (the Invoke/Builtins/dispatch
scheduler-extraction arc), which made `Scheduler` the sole implementor.

**Requires:** none — the sole-implementor precondition has shipped.

**Unblocks:** none.

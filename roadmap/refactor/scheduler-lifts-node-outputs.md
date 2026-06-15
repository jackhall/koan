# Scheduler lifts node outputs

A node's continuation produces its value at the node's own frame lifetime; the
scheduler promotes it across the dep edge.

**Problem.** Every per-node continuation is typed at `'run`. `NodeCont<'a>`
([`src/machine/execute/outcome.rs`](../../src/machine/execute/outcome.rs)) is
`Box<dyn for<'s> FnOnce(&SchedulerView<'a,'s>, &[Result<Carried<'a>,KError>], usize) -> Outcome<'a> + 'a>`
— its output `Outcome<'a>` / `Carried<'a>` is bound to the run lifetime, even
though the value a node produces is born in the node's own per-call frame
(`CallFrame::cart`, [`src/machine/execute/nodes.rs`](../../src/machine/execute/nodes.rs)).
The Done arm already relocates that value out of the dying frame —
`compute_done_output(output, frame, dest_arena, …)`
([`src/machine/execute/scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs))
lifts via `lift_kobject` ([`src/machine/execute/lift.rs`](../../src/machine/execute/lift.rs))
into the consumer's arena — but because the value is typed `'run` *before* that
lift, the lift is a same-lifetime re-home rather than a per-node→consumer
promotion. This uniform-`'run` output is one of the two reasons `'run` is smeared
across every `scheduler/` file (the other is the continuation's *captures*,
addressed by [Workload-independent DAG runtime](workload-independent-dag-runtime.md)).

**Acceptance criteria.**

- A node continuation's return type is bound to the per-step frame lifetime `'s`
  (the `'a: 's` brand already threaded through `SchedulerView` / `BuiltinFn`), not
  `'run`.
- Lift runs as the scheduler's own step, at the Done / dep-delivery boundary,
  promoting a node's `'s`-bound output to the consuming node's lifetime — the
  scheduler decides *when and where* a value is relocated.
- The Koan-value-aware relocation (`lift_kobject` / `compute_done_output`) is
  reached through a workload-provided hook the scheduler calls, not inlined into
  the scheduler loop as direct knowledge of `KObject` / `KType`.
- TCO frame reuse, the escape gate, and the MATCH `outer_frame` chain still hold;
  the Miri audit slate stays cycle-free with zero UB and zero process-exit leaks.

**Directions.**

- *Output lifetime target — decided.* The continuation output binds to the
  per-step frame lifetime `'s`, not `'run`; the run `'a` is reached only by the
  scheduler's lift at delivery.
- *Lift policy vs mechanism split — open.* How to factor `lift_kobject` /
  `compute_done_output` so the scheduler owns policy (dying-frame detection,
  destination arena) and the workload owns the mechanism (KObject-invariant
  relocation). Recommended: a workload trait method called at the delivery
  boundary, returning the promoted value, with the policy left in the scheduler.
- *Contract enforcement placement — open.* `compute_done_output` also re-anchors
  the erased return contract and enforces the declared return type at Done. Whether
  that rides the same lift hook or splits into a separate workload Done-hook.

## Dependencies

This is the first of a pair: it shrinks the continuation's *output* lifetime;
[Workload-independent DAG runtime](workload-independent-dag-runtime.md) then erases
its *captures* and evicts the remaining Koan-semantic state.

**Requires:** none — foundation.

**Unblocks:**
- [Workload-independent DAG runtime](workload-independent-dag-runtime.md) — the
  output-lifetime shrink is the prerequisite half of confining `'run`.

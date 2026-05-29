# Ping-pong reserve frame for stateful eager-subs resumes

Recover per-iteration `CallArena` allocation savings on the stateful
keyworded / FunctionValueCall resume paths by carrying a one-deep
reserve frame across Tail Replace.

**Problem.** Every stateful eager-subs resume invocation allocates a
fresh `CallArena`. The
[`invoke_to_step_pinned`](../../src/machine/execute/scheduler/finish.rs)
helper at
[`stateful_keyworded_finish`](../../src/machine/execute/scheduler/dispatch.rs),
[`stateful_fn_value_resume_eager_subs`](../../src/machine/execute/scheduler/dispatch.rs),
and
[`stateful_install_fn_value_eager_subs_track`](../../src/machine/execute/scheduler/dispatch.rs)'s
install-time short-circuit holds a sibling clone of
`self.active_frame` across the synchronous invoke so
`try_take_reusable_frame_for_tail` sees `strong_count >= 2` and
refuses the reset — that refusal is what closes the tree-borrows hole
where resetting the slot's only Rc would deallocate the arena
`scope: &'a Scope<'a>` lives in. Refusal costs one
`CallArena::new(outer, None)` per resume invoke: a fresh
`CallArena` shell plus its initial `RuntimeArena`. For deep recursion
that parks on an eager sub each iteration (every iteration of
`recursive_tagged_match_no_uaf`'s `(HOP (Bit (zero null)))` shape),
this is one struct + one Rc heap allocation per iteration that the
legacy path also paid via its sibling Bind slot's frame clone, but
which the stateful driver's per-edge advancement model could otherwise
skip.

**Impact.**

- Steady-state allocation on the stateful keyworded /
  FunctionValueCall eager-subs recursive loop is one `RuntimeArena`
  per iteration (the inner arena `try_reset_for_tail` installs);
  the `CallArena` shell and its Rc reuse across iterations. After
  warmup, no `CallArena::new` calls on the recursion path.
- Recursive workloads that park on per-iteration eager-sub
  resolution stop fragmenting the allocator with per-iteration
  `CallArena` shell allocations. Useful as a baseline measurement
  point ahead of the stdlib's recursive-collection benchmarks.
- A documented shape (one-deep reserve, rotated at Tail Replace)
  that other scheduler subsystems can borrow if a similar
  pin-vs-reuse tradeoff surfaces (e.g. future Combine-shaped
  resume sites).

**Directions.**

- **Reserve location — decided.** Add a
  `reserve_frame: Option<Rc<CallArena>>` field to
  [`Node`](../../src/machine/execute/nodes.rs). Per-slot reserve
  drops naturally when the slot finalizes (end of recursion), needs
  no separate cleanup pass, and stays out of the `Scheduler` surface.
  Scheduler-side reserve pool was the alternative; rejected because a
  single global slot would interact badly with concurrent recursive
  chains and a multi-slot pool re-introduces the bookkeeping the
  per-slot approach avoids.

- **Rotation point — decided.** At the
  [`NodeStep::Replace` arm in `execute.rs`](../../src/machine/execute/scheduler/execute.rs),
  when `new_frame: Some(f)`, before the existing `drop(prev_frame)`,
  rotate: `slot.reserve_frame = prev_frame.take()`, dropping whatever
  was previously in the reserve (which is two-iterations-old by
  construction, no live protector). The Replace then installs `f` as
  `slot.frame` as today.

- **Pinned-invoke swap — decided.** Rewrite `invoke_to_step_pinned`
  (or add a sibling `invoke_to_step_with_reserve`) so that when
  `slot.reserve_frame` is `Some`, the helper:
  1. Pins `self.active_frame` via a local clone (keeps `scope` alive
     across bind, same as today).
  2. Swaps the reserve into `self.active_frame`:
     `self.active_frame = slot.reserve_frame.take()`.
  3. Calls `invoke_to_step`. Inside,
     `try_take_reusable_frame_for_tail` takes the now-active reserve
     frame (`strong_count == 1`, no other clone — the slot's frame
     field is the pinned local, the reserve is uniquely held), the
     reset succeeds (the reserve's scope is two iterations dead, no
     protector chain holds it), and the body runs in the reset arena.
  4. After `invoke_to_step` returns, restores
     `self.active_frame = local_pin` so the Replace path's
     `mem::replace` at
     [`execute.rs:66`](../../src/machine/execute/scheduler/execute.rs)
     sees the slot's frame in `active_frame` as today.

  When `slot.reserve_frame` is `None` (first iteration, before the
  reserve fills), the helper falls back to today's pin-only behavior.

- **Reserve warmup — decided.** Don't pre-allocate. The reserve
  fills naturally at the second Tail Replace (the first Replace has
  no prior frame to rotate in; the second's prev_frame goes into the
  reserve, which iteration 3's resume reuses). First two iterations
  pay the existing pin cost; iteration 3+ reuses. Pre-allocating to
  cut the warmup tail was considered; rejected because the cost is a
  fixed two-iteration latency rather than a per-iteration one.

- **Safety witness — open.** The roadmap item ships when Miri
  full-slate-green on `recursive_tagged_match_no_uaf` confirms the
  reset of the reserve is sound under tree borrows. The reasoning
  above (two-iteration gap puts the reserve's scope past any live
  protector) is the design argument; Miri is the structural witness.
  No additional slate test needed — the existing
  `recursive_tagged_match_no_uaf` exercises the exact pattern at
  every iteration.

- **Allocator measurement — deferred.** A microbenchmark recording
  per-iteration allocation count on the
  `recursive_tagged_match_no_uaf` shape (or a deeper-recursion
  fixture sized to the eventual stdlib benchmark) would quantify the
  win. Defer until the stdlib benchmark infrastructure exists, then
  capture the before/after numbers as part of that work — the
  benchmark, not this item, is the right place to log the data.

## Dependencies

**Requires:**


**Unblocks:** none.

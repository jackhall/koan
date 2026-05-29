# Canonical doc for the per-call arena protocol

Write `design/per-call-arena-protocol.md` as the single named owner of
the `Rc<CallArena>` lifecycle that today spans five design docs and
ten source files without a source-level home. The carriers are
correctly distributed across `KObject`, `Scheduler`, and `arena`; the
contract each participant upholds is what's missing.

**Problem.** The per-call arena protocol connects:

- which `KObject` carriers anchor a per-call arena (the `frame:
  Option<Rc<CallArena>>` field on `KFunction`, `KFuture`, and
  `KTypeValue(Module)` in
  [`kobject.rs`](../../src/machine/model/values/kobject.rs));
- how lift decides to attach an anchor
  ([`lift.rs`](../../src/machine/execute/lift.rs));
- the alloc cycle gate that redirects self-referential allocations
  ([`arena.rs`](../../src/machine/core/arena.rs));
- the active-frame propagation through `Scheduler::active_frame` /
  `SchedulerHandle::current_frame` / `with_active_frame`
  ([`scheduler.rs`](../../src/machine/execute/scheduler.rs),
  [`scheduler_handle.rs`](../../src/machine/core/kfunction/scheduler_handle.rs));
- the `outer_frame` chain for builtin-built frames (MATCH / TRY /
  EVAL in [`match_case.rs`](../../src/builtins/match_case.rs),
  [`try_with.rs`](../../src/builtins/try_with.rs),
  [`eval.rs`](../../src/builtins/eval.rs),
  [`module_def.rs`](../../src/builtins/module_def.rs));
- TCO frame reuse and the ping-pong reserve mechanism
  ([`finish.rs`](../../src/machine/execute/scheduler/finish.rs),
  [`execute.rs`](../../src/machine/execute/scheduler.rs),
  [`nodes.rs:NodeStep::Replace::reserve_frame`](../../src/machine/execute/nodes.rs)).

It is described across five design docs:
[`memory-model.md`](../../design/memory-model.md) (primary home —
Closure escape, Cycle gate, Tail-step frame reuse, Per-call-frame
chaining, Ping-pong reserve),
[`functors.md`](../../design/typing/functors.md) (per-call type-side
bind + `with_active_frame` for deferred return Combines),
[`modules.md`](../../design/typing/modules.md) (`USING ... SCOPE`
allocates in the call-site arena),
[`execution-model.md`](../../design/execution-model.md) (TCO frame
reuse, `reserve_frame` ping-pong, `reinstall_with_frame`), and
[`error-handling.md`](../../design/error-handling.md) (TCO frame
collapse under tail call).

No source file owns the contract: the carriers belong in `kobject`,
allocation in `arena`, scheduler state on `Scheduler`. The
distribution is correct; the missing artifact is a single page that
enumerates the participants and the obligation each upholds. A
reader investigating the protocol assembles it from five docs and
ten source files instead of one page.

**Impact.**

- One canonical page enumerates the per-call arena protocol's
  participants explicitly: which `KObject` variants carry `frame:
  Option<Rc<CallArena>>`, what each carrier's anchor obligation is,
  when allocation routes through the cycle gate, how active-frame
  propagation threads through the scheduler stack, and how the
  outer-frame chain works for builtin-built frames.
- The five existing docs link to the canonical page instead of
  restating fragments; each doc's per-call-arena prose shrinks to a
  single cross-link.
- New contributors reading the memory model land on one page rather
  than reconstructing the protocol from five.
- Future per-call-arena work (any new carrier type, any change to
  the cycle gate, any TCO frame-management change) updates one doc
  by partition.

**Directions.**

- **Doc-only seam — decided per Pass 11.** This protocol is a
  documented-but-correctly-distributed case (not a hidden code
  seam). The candidates analysis tried both a doc-only seam and a
  trait-anchored code seam; the trait seam adds indirection where
  today there's just a field. The doc-only seam ships first; the
  trait seam is rejected unless a future change makes the field
  pattern untenable.
- **Carrier set named explicitly — decided.** The page must list
  the three carrier types (`KFunction`, `KFuture`,
  `KTypeValue(Module)`) and the `frame: Option<Rc<CallArena>>`
  discriminator that ties them together. Future carriers that grow
  the same field join the list.
- **Inbound link rewrites — decided.** The five existing docs each
  keep their topic-specific content; their per-call-arena prose
  trims to a `see [design/per-call-arena-protocol.md]` reference.

## Dependencies

**Requires:** none.

**Unblocks:** none.

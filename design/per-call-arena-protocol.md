# Per-call arena protocol

The contract for [`Rc<CallArena>`](../src/machine/core/arena.rs): which
[`KObject`](../src/machine/model/values/kobject.rs) variants carry a
per-call anchor, how
[`lift_kobject`](../src/machine/execute/lift.rs) decides to attach one,
how the `alloc_object` cycle gate routes self-referential allocations,
how the [scheduler](../src/machine/execute/scheduler.rs) propagates the
active frame, how builtin-built frames chain the call-site frame
through `outer_frame`, and how the TCO step reuses the frame shell.
The participants live in `KObject` (carriers), `arena.rs` (allocation
/ storage), and `Scheduler` (active-frame plumbing); this page is the
single named owner so a reader investigating the protocol lands here
rather than reconstructing it from five docs and ten source files.

## Carriers

Three `KObject` variants embed an `Option<Rc<CallArena>>` lifecycle
anchor:

- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>)` — a
  closure value. Anchor is `Some(_)` when the captured definition
  scope lives in a per-call arena, `None` when it lives in run-root.
- `KObject::KFuture(KFuture<'a>, Option<Rc<CallArena>>)` — a future
  value. The `KFuture` embeds `&KFunction`, a bundle, and a parsed
  `KExpression` whose `Future(&KObject)` parts can independently
  borrow into per-call storage; the anchor pins the per-call arena
  alive when any of those borrows points there.
- `KObject::KTypeValue(KType::Module { module, frame })` — a
  first-class module value. `frame` is the per-call `Rc<CallArena>`
  of the functor call that minted the module; `None` for top-level
  `MODULE` declarations.

A fourth participant lives on `CallArena` itself: `outer_frame:
Option<Rc<CallArena>>` chains the parent per-call frame when a
builtin-built frame's child scope's `outer` points into per-call
memory (MATCH / TRY / EVAL / MODULE under a functor call). The two
anchor positions are distinct: the `KObject` anchor keeps the arena
alive for an *escaped value*; `outer_frame` keeps it alive for an
*outer-scope lookup* the new frame's child scope performs at run time.

Future carriers that need to extend the lifetime of a per-call arena
join the list by growing the same `Option<Rc<CallArena>>` field.

## Lift-time anchor decision

`lift_kobject` runs when a per-call value is extracted into a
destination arena — typically a closure returned from its defining
frame, a module value flowing out of a functor body, or a future
referencing per-call state. Per carrier:

- **`KFunction`.** Compare `f.captured_scope().arena` to the dying
  frame's arena pointer. Match → clone the dying frame's `Rc` onto the
  lifted value; mismatch → no `Rc`.
- **`KTypeValue(Module)`.** Compare `m.child_scope().arena` to the
  dying frame's arena pointer; same rule.
- **`KFuture`.** Run a targeted membership walk
  (`kfuture_borrows_dying_arena`) that asks the dying arena's
  `owns_object` side-table whether each embedded `Future(&KObject)`
  borrow points into it, recursing through nested expressions,
  list/dict literals, and bundle arg values; the embedded function
  reference is checked via the same captured-scope-arena equality test
  the `KFunction` arm uses. Anchor only fires when at least one
  descendant actually borrows into the dying arena. `RuntimeArena`
  records every allocated `KObject`'s stable address (typed-arena
  allocations don't move) in an addresses-only `Vec<usize>` so the
  membership query is a single linear scan with no deref or borrow.

Composite variants (`List`, `Dict`) recurse with a `needs_lift`
short-circuit: when no descendant needs anchoring, the existing
`Rc<Vec>` / `Rc<HashMap>` is cloned in place rather than rebuilt.
Koan's collection-immutability contract is what makes the structural
sharing safe.

### Fast path

If a dying arena allocated zero `KFunction`s (`functions_is_empty`),
no descendant `&KFunction` can point into it, and `lift_kobject`
collapses to a plain `deep_clone`. The gate is sufficient *because*
KFutures do not escape as values today: every borrow into the dying
arena that the slow path checks (KFunction captured-scope,
KFuture-embedded function ref and parsed-expression
`Future(&KObject)` parts, Module child-scope) traces back to a
KFunction, so "no KFunction allocated here" implies "no descendant
borrows into here." Once KFutures become first-class values that can
ride through `Future(&KObject)` parts independently of any KFunction,
the gate must add a no-unanchored-KFuture-descendant clause; the slow
path's KFuture arm already carries the membership-walk machinery the
fast path would defer to.

## Cycle gate on `alloc_object`

The anchor mechanism creates a self-referential shape if a composite
carrying an escaping closure is re-allocated into the same per-call
arena it already anchors to: the arena's storage holds the composite,
the composite holds the `Rc<CallArena>`, and the `Rc` holds the arena.
Neither side can drop. The case shows up when a body returns a
List / Dict / Tagged / Struct holding a closure — the lift-on-return
machinery attaches the per-call frame's `Rc` to the closure, then a
re-allocation of the composite (via `value_pass`, `Combine`, etc.)
lands the composite back in the per-call arena.

`RuntimeArena` carries an `escape: Option<*const RuntimeArena>` set by
`CallArena::new` to the outer scope's arena address. `alloc_object`
walks the incoming value's composite tree (`obj_anchors_to`, mirroring
`KObject::deep_clone`'s shape) and, on finding any `Rc<CallArena>`
whose `arena()` is `self`, redirects the allocation up to the escape
arena — where the same `Rc` is no longer self-referential. The
redirect is a single `Option`-check on every per-call `alloc_object`;
run-root has `escape: None` and short-circuits, since the
`Rc<CallArena>` shapes the gate looks for can only point at per-call
arenas by construction. The escape pointer is stable for the per-call
arena's life because `CallArena::new` heap-pins the outer arena via
`Rc`, and the outer always outlives this inner per the lexical-scoping
invariant.

`KObject` and `KType` go through the single cycle-gated `alloc` entry
via the `CycleGated` trait; `KFunction`, `Scope`, `Module`, and
`Signature` use un-gated `alloc_*` methods because none of them can
hold a self-targeting `Rc<CallArena>`.

## Active-frame propagation

The scheduler exposes the currently running slot's frame to code that
needs to capture it ([builtin-built frame chaining](#outer-frame-chain-for-builtin-built-frames)
below, deferred sub-Dispatch under a per-call frame). Three pieces of
state live on `Scheduler`:

- **`active_frame: Option<Rc<CallArena>>`** — frame of the slot
  currently being executed. Read through
  [`SchedulerHandle::current_frame`](../src/machine/core/kfunction/scheduler_handle.rs);
  written only by `enter_slot_step` / `exit_slot_step` (the RAII
  bracket around every iteration of `Scheduler::execute`) and by the
  ping-pong reserve consumer.
- **`active_reserve: Option<Rc<CallArena>>`** — the slot's reserve
  frame, drained from `Node::reserve_frame` through `enter_slot_step`
  and consumed by `invoke_to_step_pinned` (see [§ Ping-pong reserve
  frame](#ping-pong-reserve-frame)).
- **`SchedulerHandle::with_active_frame(frame, f)`** — temporarily
  installs `frame` as `active_frame` for the duration of a closure
  call. Used by `KFunction::invoke` to spawn a deferred return-type
  sub-Dispatch under the per-call frame so the sub-Dispatch's scope
  resolves against the per-call type-side bind (see
  [typing/functors.md § Deferred return-type elaboration](typing/functors.md#deferred-return-type-elaboration)).

`Scheduler::execute` *moves* `node.frame` into `self.active_frame`
(no clone) for the duration of each step. That single-ownership
discipline is what lets the tail-reuse path detect "nothing escaped"
via `Rc::strong_count == 1`: a clone visible to `strong_count` is a
real escape. Sub-Dispatch / sub-Bind / sub-Combine slots spawned via
`add()` inherit `active_frame` so they see the right ancestor for
their own chaining decisions.

## Outer-frame chain for builtin-built frames

A user-fn call's per-call frame is anchored by lexical scoping: the
new frame's child scope's `outer` is the FN's *captured* scope
(run-root for top-level FNs), which outlives every per-call frame.
Builtins that build their own per-call frame don't always have that
property. The frame-chain `Rc` on `CallArena` (`outer_frame:
Option<Rc<CallArena>>`) keeps the parent frame alive whenever the
child's `outer` points into per-call memory.

Each builtin clones `sched.current_frame()` into its `CallArena::new`
call:

- `match_case.rs` — MATCH constructs a frame whose child scope's
  `outer` is the **call-site** scope so free names in the arm body
  resolve against the surrounding call.
- `try_with.rs` — TRY-WITH dispatches each branch under a frame
  chained to the TRY call site so the branch body's free names
  resolve through the surrounding call.
- `eval.rs` — EVAL builds a per-call frame for the evaluated
  expression.
- `module_def.rs` — MODULE captures `sched.current_frame()` so the
  module's child scope chains through the call site (relevant when a
  functor body declares an inner MODULE).

Top-level FN invokes pass `None` to `CallArena::new` (their captured
chain ends in run-root, which outlives the run; no chain is needed and
TCO recursion stays bounded). Field declaration order on `CallArena`
is load-bearing: `arena` is declared before `outer_frame`, so the
auto-derived `Drop` tears down this frame's arena *before* releasing
the parent Rc — inner pointers die before the outer storage they may
reference.

## TCO frame reuse

Each TCO step would otherwise drop the previous slot's `CallArena` and
allocate a fresh one — six typed-arena pools, an
`Rc<RefCell<Vec<usize>>>`, an alloc'd child `Scope`, and the
`Rc<CallArena>` box itself per iteration. `CallArena::try_reset_for_tail`
reuses the shell across iterations: swap the inner `RuntimeArena` for
a fresh empty one, re-allocate the child `Scope` into it, re-link
`outer` to the new call's captured scope. The `Rc`, the heap-pinned
arena address, and the slot's `frame` field carry over unchanged.

Two structural invariants make the reset sound:

- **No escape.** `Rc::get_mut` succeeds iff no other `Rc` to the frame
  exists. Any escaped value (a closure carrying `Some(Rc)`, a list
  element holding one, a sub-Dispatch slot that cloned `active_frame`)
  keeps `strong_count > 1` and refuses the reset, falling through to
  `CallArena::new`. The escape gate's correctness depends on
  `Scheduler::execute` moving `node.frame` into `self.active_frame`
  for the duration of each step — see [§ Active-frame propagation](#active-frame-propagation).
- **No live external refs into the arena's storage.** By the time TCO
  Replace fires, every sub-Dispatch slot the previous body spawned has
  terminalized and freed, and the slot's `dep_edges` are cleared. The
  only remaining references into the old arena's contents live in the
  slot's own scope, which we're about to rebind. Resetting the storage
  drops the old contents safely.

Frame reuse is what makes deep tail recursion truly constant-memory —
both in the scheduler's slot table (the `Tail` rewrite alone) and on
the heap (the reset turns over arena storage in place rather than
allocating per step). `SchedulerHandle::try_take_reusable_frame_for_tail`
takes the active frame, refuses to hand it out if any clone exists,
and otherwise lets `KFunction::invoke` reset the frame in place.
Frames carrying an escaped closure (or any other clone of the `Rc`)
fall through to a fresh `CallArena::new`, preserving snapshot
semantics for the escaped value.

### MATCH frame lifetime under tail recursion

When a user-fn recurses through a `MATCH` arm, the recursive call sits
inside the MATCH-built per-call frame, not the user-fn's own frame.
MATCH clones the user-fn's frame Rc onto its own frame's
`outer_frame`, so the user-fn frame's `strong_count` is `> 1` for the
duration of the arm body. The TCO Replace at the recursive call
therefore refuses in-place reset on that step and routes through
`CallArena::new` — the chained `Rc` is a real alias. Cross-step reuse
resumes one iteration later once the MATCH frame is itself replaced
by the next tail step and its `outer_frame` Rc drops.

The bound the `chained_user_fn_tail_calls_reuse_one_slot` and
`match_driven_tail_recursion_completes` tests pin is: the user-fn
frame is alive across exactly one MATCH-arm iteration at a time, and
the call chain collapses to one scheduler slot via the `Tail` rewrite
even when reset refuses on individual MATCH-arm steps. Without the
chained Rc, the recursive arm body's `outer` pointer into the dying
frame would dangle on TCO Replace.

## Ping-pong reserve frame

The stateful dispatch driver's eager-subs resume / install-time
short-circuit sites — keyworded and `FunctionValueCall` invocations
routed through `invoke_to_step_pinned` — hold the only `Rc<CallArena>`
for the arena that the running `scope` borrows into. Pinning that
frame across the synchronous invoke keeps `strong_count >= 2`, which
forecloses tail-reuse on the slot's only frame Rc — without the pin,
`try_reset_for_tail` would deallocate the arena while `scope`'s
tree-borrows protector is still live. The cost is one
`CallArena::new` per resume invoke that the legacy keyworded path
could otherwise have skipped.

To recover that allocation, the slot carries a per-iteration **reserve
frame** in `Node::reserve_frame` that ping-pongs across
`NodeStep::Replace`:

- **Replace arm in `execute.rs`.** On a new-frame Replace, drop the
  (now two-iterations-old) reserve, rotate the post-step frame into
  `slot.reserve_frame`, install the new frame as `slot.frame`. First
  iteration's reserve stays `None`; second iteration fills it;
  iteration 3+ has a reserve to consume.
- **Reserve-consuming arm in `invoke_to_step_pinned`.** When the
  slot's reserve is `Some`, the helper pins `active_frame` (the
  slot's current frame) via a local clone — still anchoring `scope` —
  and swaps the reserve into `active_frame`. The reserve's
  `strong_count` is 1 (only the slot's `reserve_frame` field held it,
  drained through `enter_slot_step` into `Scheduler::active_reserve`),
  so `try_take_reusable_frame_for_tail` succeeds, the reset lands,
  and the body runs in the reset arena. After the invoke returns, the
  local pin is swapped back into `active_frame` so the Replace arm
  reads the slot's frame as today.

The dispatcher reaches the slot's reserve / active-frame state through
the narrow [`DispatchCtx`](../src/machine/execute/dispatch/ctx.rs)
facade (see [execution-model.md § The dispatcher / scheduler
boundary](execution-model.md#the-dispatcher--scheduler-boundary)) —
`sched.active_reserve_take()` drains the reserve, and
`sched.active_frame_replace(...)` performs the local pin/swap. The
`active_frame` / `active_reserve` fields themselves stay
`pub(in execute::scheduler)`; the accessor surface is what dispatch
sees.

The two-iteration gap is the safety witness: when iteration N consumes
the reserve, the reserve's scope was the active scope on iteration
N-2 and is past every live tree-borrows protector by the time
iteration N's invoke fires. Miri full-slate green on
`recursive_tagged_match_no_uaf` — which exercises exactly this pattern
at every iteration — under `MIRIFLAGS=-Zmiri-tree-borrows` is the
structural confirmation.

Steady-state allocation on the stateful keyworded /
`FunctionValueCall` recursive loop is one `RuntimeArena` per iteration
(the inner arena `try_reset_for_tail` installs); the `CallArena`
shell and its `Rc` reuse across iterations after the first
two-iteration warmup.

## Slot-table re-anchor

Storing the slot's per-call frame in the scheduler's slot table
requires one re-anchor at install time: the slot-table type uses `'a`
(the run lifetime), but `Rc<CallArena>::scope()` returns `&'p
Scope<'p>` bounded by the local receiver.
`NodeStore::reinstall_with_frame` performs that re-anchor through
[`CallArena::anchored_parts`](../src/machine/core/arena.rs) — the single
re-anchor method also used by the MATCH / TRY-WITH builtins and
`KFunction::invoke` — under the witness "the `Rc<CallArena>` stays in the same
`Node` payload as the `&'a Scope<'a>` it produced": as long as the slot owns the
Rc, the arena heap-pinning that backs the child scope pointer outlives every read
through the `'a` reference. Any previous frame in the slot must have been
removed by a prior `take_for_run`, so there is no shadow alias being
silently overwritten.

## Cross-doc context

The protocol surfaces from five concerns; each owning doc keeps its
topic-specific narrative and cross-links here for the protocol
mechanics:

- [memory-model.md](memory-model.md) — value ownership through
  `RuntimeArena` / `CallArena`, the storage shape, scoping, and
  lifetime erasure that this protocol sits on top of.
- [execution-model.md](execution-model.md) — the dispatch / TCO
  pipeline whose `Tail` rewrite drives `try_reset_for_tail`.
- [typing/functors.md](typing/functors.md) — the per-call type-side
  bind and the `with_active_frame` deferred return-type Combine.
- [typing/modules.md](typing/modules.md) — `USING … SCOPE` allocating
  in the call-site arena so a forwarded bind or window-surfaced
  member outlives the block.
- [error-handling.md](error-handling.md) — TCO frame collapse as
  observed in error traces.

## Verification

- `unanchored_kfuture_no_arena_borrow_does_not_anchor` and
  `unanchored_kfuture_with_arena_borrow_does_anchor` cover both sides
  of the targeted KFuture anchor.
- `fast_lane_closure_escapes_outer_call_and_remains_invocable` and
  `fast_lane_escaped_closure_with_param_returns_body_value` confirm a
  closure returned from its defining frame remains invocable.
- `alloc_object_redirects_self_anchored_value_to_escape_arena` locks
  in the cycle gate: a value carrying an `Rc<CallArena>` whose
  `arena()` is the receiving arena allocates into the escape arena
  instead, with the per-call arena's storage left untouched.
- `recursive_tagged_match_no_uaf` runs a user-fn that recurses through
  a `Tagged` parameter via MATCH, exercising the `outer_frame` chain
  that keeps the call-site arena alive across TCO replace.
- `call_arena_try_reset_for_tail_round_trip` and
  `call_arena_try_reset_for_tail_refuses_when_aliased` pin the
  in-place reset: a unique `Rc` resets and re-binds correctly against
  the new outer scope; an aliased `Rc` (the escape case) refuses with
  the frame's arena pointer unchanged.
- `chained_tail_calls_reuse_frames` asserts that a chain of user-fn
  tail calls (`AA → BB → CC → DD → PRINT`) bumps the scheduler's
  tail-reuse counter and collapses to one slot.
- `repeated_user_fn_calls_do_not_grow_run_root_per_call` asserts 50
  ECHO calls grow the run-root arena by exactly 50 — one lifted
  return value per call, with all per-call scaffolding freed at call
  return.
- The audit slate runs cycle-free across every unsafe site that
  routes through the protocol under `MIRIFLAGS=-Zmiri-tree-borrows`
  with zero UB and zero process-exit leaks. The canonical slate list
  lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

- (none)

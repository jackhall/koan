# Scope handles and verification

The slot-table scope handle, the seed-side re-anchor, cross-doc context, and the
verification slate. Part of the [per-call region protocol](README.md).

## Slot-table scope handle

A scheduler slot stores its scope as a lifetime-free
[`NodeScope`](../../src/machine/execute/nodes.rs), not a raw `&'a Scope<'a>`, so the node it sits on
pins no `'run` through its scope. The handle rides a grouped `NodePayload` (the scope handle plus the
node's lexical chain) alongside the slot's frame. Both arms are **cart-witnessed** — re-projected
from the slot's live frame at read, never re-anchored at a free `'run`:

- `Yoked` carries no payload at all: the slot's scope *is* its own per-call cart's scope, re-read
  from the frame at the read boundary. Single-cart, because the slot's own `Frame::cart`
  `Rc<CallFrame>` is the sole liveness witness, so there is no second `Rc` clone and no contention
  with `try_reset_for_tail`'s `strong_count == 1` TCO reuse check.
- `YokedChild(SealedExtern<ScopeRefFamily>)` holds a `&'static Scope` carrier to a block scope a
  builtin allocated in a cart *ancestor* region (an `InScope` body — USING / MODULE / SIG / TRY),
  opened at read through the rank-2 `SealedExtern::open` at a `for<'b>` brand against the slot's frame
  `Rc`, sound because the cart's `FrameStorage.outer` chain pins that ancestor region for as long as
  the slot holds the cart. It differs from `Yoked` only in that the child scope differs from the cart's
  own scope, so it needs a stored carrier.

The funnel [`resolve_node_scope`](../../src/machine/execute/runtime/submit.rs) decides the arm in
order: a pointer test (`std::ptr::eq(active_frame.scope(), scope)`) routes a frame's-own-child slot
to `Yoked`; a walk of the active cart's scope `outer` chain that reaches `scope`'s region routes a
cart-ancestor block scope to `YokedChild`, erasing the borrow through `SealedExtern::<ScopeRefFamily>::erase`; the
frameless top-level run root routes to `Yoked` via the `run_frame` cart that adopts it (the slot's
cart is that `run_frame`). The two residual fall-throughs are `unreachable!` — an instrumented
whole-suite spike confirmed every framed submission resolves to `Yoked` / `YokedChild` and every
frameless one to the run root. Storing an erased handle rather than a live `&'run` keeps the borrow
honest across a TCO `try_reset_for_tail`: nothing persisted points into the reset region.

The read boundary hands a slot's scope to a closure on demand, not as a stored free `&'run`:
[`with_node_scope`](../../src/machine/execute/dispatch/ctx.rs) opens it per use at a `for<'b>` brand —
a `YokedChild` slot opens its stored `&'static Scope` carrier through `SealedExtern::open` (witnessed
by the frame `Rc`, sound because the cart pins the ancestor region; the open carries no `unsafe` of
its own); a `Yoked` slot re-reads from the live `active_frame` cart via
[`CallFrame::with_scope`](../../src/machine/core/arena.rs), the same rank-2 `open`. Because the
`&Scope<'b>` is confined to the closure, storing it past the frame is a compile error rather than a
fabrication; `Scope<'a>` invariance rides structurally on the returned `Scope`, so the brand needs no
separate struct. Bodies / finishes / the dispatch engine no longer thread a `scope` parameter — they
call `current_scope()`; the genuine run-scope methods (`dispatch_in_scope` /
`dispatch_in_scope_with_chain` / `enter_block`) keep their `&'a Scope` argument.

The post-step loop in `Scheduler::execute` reads the just-finished step's scope through a
`PostStep` token returned by `exit_slot_step`, derived from the slot's *returned* frame
(`prev_frame`) rather than the ambient `active_frame` — an in-step invoke can swap the ambient
frame, so the returned value is the authoritative source. A within-step frame lifetime `'step`
(`'a: 'step`) threads `classify_dispatch` → `SchedulerView` → `BuiltinFn` → the scheduler's write
primitives, lifting to the run `'a` only at the `lift_kobject` Done boundary.

## Seed-side re-anchor

The MATCH / TRY arm seeds and [`run_user_fn`](../../src/machine/core/kfunction/exec.rs)
bind their `it` / parameters — values whose type carries the caller's `'a`, deep-cloned into the
frame region — inside [`CallFrame::with_scope`](../../src/machine/core/arena.rs), which opens the
child scope at a `for<'b>` brand. A seed **relocates** its caller-`'a` value into the opened scope's
own region through the substrate (the erasing `alloc_object`, which forgets the caller lifetime and
re-homes the value at the frame region) before binding it, so the value lands at the brand and the
seed fabricates no free `&'a`. The deferred-return-type elaboration takes the same `with_scope` read
and re-homes its elaborated `KType` into the captured-scope region inside the open. The whole
re-anchor carries no `unsafe` of its own — only the substrate's single retype.
Arm and body statements then dispatch through the framed scheduler write primitives
(`dispatch_in_active_frame`, `dispatch_body`), which
derive the scope from the active frame and store `Yoked`, so the seed itself persists no
fabricated `&'a`.

## Cross-doc context

The protocol surfaces from five concerns; each owning doc keeps its
topic-specific narrative and cross-links here for the protocol
mechanics:

- [memory-model.md](../memory-model.md) — value ownership through
  `KoanRegion` / `CallFrame`, the storage shape, scoping, and
  lifetime erasure that this protocol sits on top of.
- [execution/README.md](../execution/README.md) — the dispatch / TCO
  pipeline whose `Tail` rewrite drives `try_reset_for_tail`.
- [typing/functors.md](../typing/functors.md) — the per-call type-side
  bind and the deferred return-type dep-finish.
- [typing/modules.md](../typing/modules.md) — `USING … SCOPE` allocating
  in the call-site region so a forwarded bind or window-surfaced
  member outlives the block.
- [error-handling.md](../error-handling.md) — TCO frame collapse as
  observed in error traces.

## Verification

- `unanchored_kfuture_no_arena_borrow_does_not_anchor` and
  `unanchored_kfuture_with_arena_borrow_does_anchor` cover both sides
  of the targeted KFuture anchor.
- `fast_lane_closure_escapes_outer_call_and_remains_invocable` and
  `fast_lane_escaped_closure_with_param_returns_body_value` confirm a
  closure returned from its defining frame remains invocable.
- `alloc_object_redirects_self_anchored_value_to_escape_arena` locks
  in the cycle gate: a value carrying an `Rc<FrameStorage>` whose
  `region()` is the receiving region allocates into the escape region
  instead, with the per-call region's storage left untouched.
- `recursive_tagged_match_no_uaf` runs a user-fn that recurses through
  a `Tagged` parameter via MATCH, exercising the `FrameStorage.outer`
  chain that keeps the call-site region alive across TCO replace.
- `call_arena_try_reset_for_tail_round_trip` and
  `call_arena_try_reset_for_tail_refuses_when_aliased` pin the
  in-place reset: a unique shell `Rc` resets and re-binds correctly
  against the new outer scope; a second shell `Rc` clone refuses with
  the frame's region pointer unchanged.
- `call_arena_try_reset_for_tail_allows_reset_under_escaped_storage`
  pins the storage/shell split: an escaped value pinning the
  `FrameStorage` (not the shell) does **not** foreclose reuse — the
  reset installs fresh storage while the escapee's retained Rc keeps
  the pre-reset region and its allocations alive and aliased.
- `chained_tail_calls_reuse_frames` asserts that a chain of user-fn
  tail calls (`AA → BB → CC → DD → PRINT`) bumps the scheduler's
  tail-reuse counter and collapses to one slot.
- `repeated_user_fn_calls_do_not_grow_run_root_per_call` asserts 50
  ECHO calls grow the run-root region by exactly 50 — one per-call
  argument value (`Number(7)`) per call, with all per-call scaffolding
  freed at call return. Intermediate node outputs no longer land in
  run-root: a consumed value dies with its consumer's frame, and only a
  consumer-less root drains to the run region.
- The audit slate runs cycle-free across every unsafe site that
  routes through the protocol under `MIRIFLAGS=-Zmiri-tree-borrows`
  with zero UB and zero process-exit leaks. The canonical slate list
  lives in [observe/miri_slate.md](../../observe/miri_slate.md).

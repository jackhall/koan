# Frame management

Active-frame propagation, the outer-frame chain for builtin-built frames, TCO
frame reuse, and the ping-pong reserve frame. Part of the [per-call region
protocol](README.md).

## Active-frame propagation

The scheduler exposes the currently running slot's frame to code that
needs to capture it ([builtin-built frame chaining](#outer-frame-chain-for-builtin-built-frames)
below, deferred sub-Dispatch under a per-call frame). Three pieces of
state live on `Scheduler`:

- **`active_frame: Option<Rc<CallFrame>>`** — frame of the slot
  currently being executed. Read through
  [`Scheduler::current_frame`](../../src/machine/execute/run_loop.rs);
  written only by `Scheduler::with_slot_step` (the RAII bracket
  `run_step` wraps each slot step in) and the `Scheduler::with_active_frame`
  bracket. An invoke never takes it (tail
  reuse draws from the reserve, below), so within a step it is always
  `Some` — `Node::frame` and `PostStep::prev_frame` are non-optional.
- **`active_reserve: Option<Rc<CallFrame>>`** — the slot's reserve
  frame, drained from `Node`'s `Frame::reserve` through
  `with_slot_step` and consumed by `acquire_tail_frame` (see
  [§ Ping-pong reserve frame](#ping-pong-reserve-frame)).
- **`Scheduler::with_active_frame(frame, body) -> R`** — brackets
  `frame` as `active_frame` for the duration of `body`, restoring the
  previous one on every exit path, unwind included. Used by
  [`KoanRuntime::dispatch_body`](../../src/machine/execute/runtime/submit.rs) to
  dispatch a body's non-tail statements under the body frame so each sub-slot
  inherits it as its cart (see
  [typing/functors.md § Deferred return-type elaboration](../typing/functors.md#deferred-return-type-elaboration)
  for the per-call type-side bind that motivates it).

`Scheduler::execute` *moves* `node.frame` into `self.active_frame`
(no clone) for the duration of each step. That single-ownership
discipline is what lets the tail-reuse path detect "nothing escaped":
when the just-finished active frame rotates into the slot's reserve and
a later step tries to reuse it, `try_reset_for_tail`'s `Rc::get_mut`
succeeds only at `strong_count == 1` — a clone visible to `strong_count`
(an escaped closure, a sub-Dispatch that cloned `active_frame`) is a
real escape and refuses the reset. Sub-dispatch and dep-finish slots inherit
`active_frame` so they see the right ancestor for their own chaining decisions.

## Outer-frame chain for builtin-built frames

A user-fn call's per-call frame is anchored by lexical scoping: the
new frame's child scope's `outer` is the FN's *captured* scope
(run-root for top-level FNs), which outlives every per-call frame.
Builtins that build their own per-call frame don't always have that
property. The frame-chain `Rc` on `FrameStorage` (`outer:
Option<Rc<FrameStorage>>`) keeps the parent frame's storage alive
whenever the child's `outer` points into per-call memory. The builtin
threads the chain by passing the call-site frame's `storage_rc()` into
`CallFrame::new`, which stores it on the new frame's `FrameStorage.outer`.

Each builtin clones `sched.current_frame()` into its `CallFrame::new`
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

Top-level FN invokes pass `None` to `CallFrame::new` (their captured
chain ends in run-root, which outlives the run; no chain is needed and
TCO recursion stays bounded). Field declaration order on `FrameStorage`
is load-bearing: `region` is declared before `outer`, so the
auto-derived `Drop` tears down this frame's region *before* releasing
the parent storage Rc — inner pointers die before the outer storage they
may reference. The frame shell holds its child scope as a delivery envelope
([`Delivered`](../../workgraph/src/witnessed/delivered.rs)) whose host is the
storage — one co-located carrier, built witnessed at construction. Dropping the
sealed carrier never dereferences the child pointer, so the shell needs no
drop-order rule of its own.

## TCO frame reuse

Each TCO step would otherwise drop the previous slot's `CallFrame` and
allocate a fresh one — six typed-arena pools, an
`Rc<RefCell<Vec<usize>>>`, an alloc'd child `Scope`, and the
`Rc<CallFrame>` box itself per iteration. `CallFrame::try_reset_for_tail`
reuses the shell across iterations: install a fresh `FrameStorage` (a new
empty `KoanRegion`, `outer` re-linked to the new call's captured
scope), re-allocate the child `Scope` into it. The shell `Rc` and the
slot's `frame` field carry over unchanged; the old `FrameStorage` (and
its region) drops here *unless* an escaped value still pins it, in which
case that snapshot lives on independently while the shell reuses. The
region address therefore *changes* across a reset (the fresh
`FrameStorage` is a new heap box) — no code captures an region pointer
across a reset, and for safe code the borrow checker guarantees it can't
(see the cross-reset capture invariant below).

Two structural invariants make the reset sound:

- **No live shell alias.** `Rc::get_mut` succeeds iff no other
  `Rc<CallFrame>` *shell* clone exists. An escaped value pins
  `FrameStorage`, not the shell, so it does **not** foreclose reuse:
  the swap drops the shell's reference to the old storage while the
  escapee's clone keeps that snapshot alive and aliased. Only a
  transient shell clone (a sub-Dispatch slot that cloned the shell
  `Rc`) keeps `strong_count > 1` and refuses, falling through to
  `CallFrame::new`. The gate's correctness depends on
  `Scheduler::execute` moving `node.frame` into `self.active_frame`
  for the duration of each step — see [§ Active-frame propagation](#active-frame-propagation).
- **No live external refs into the region's storage.** By the time TCO
  Replace fires, every sub-Dispatch slot the previous body spawned has
  terminalized and freed, and the slot's `dep_edges` are cleared. The
  only remaining references into the old region's contents live in the
  slot's own scope, which we're about to rebind. Installing fresh
  storage drops the old contents safely (or hands them to the escapee
  that pinned them).

**Cross-reset region capture is borrow-checker-enforced for safe code.**
`CallFrame::region()` returns an `&self`-bounded `&KoanRegion`, while
`try_reset_for_tail` takes `&mut Rc<CallFrame>`, so a live region borrow
cannot span that frame's reset — a captured pointer across the reset is
a compile error, not a discipline the code must remember.
`with_scope`'s seed binds relocate their caller value into the opened
child scope's own region through the substrate, pinned by the held frame
`Rc` — see [§ Seed-side re-anchor](scope-handles.md#seed-side-re-anchor)).

Frame reuse is what makes deep tail recursion truly constant-memory —
both in the scheduler's slot table (the `Tail` rewrite alone) and on
the heap (the reset turns over region storage in place rather than
allocating per step). The harness acquires the body's frame for the pure
`dispatch::exec::invoke` decide through `Scheduler::acquire_tail_frame(outer)`,
which reuses the slot's **reserve** cart — resetting it in place when uniquely owned —
and otherwise allocates a fresh `CallFrame::new`. Reuse draws from the
reserve, never the live active frame, so an invoke never empties the
slot's own cart. A reserve carrying an escaped closure (or any other
clone of its `Rc`) fails `try_reset_for_tail`'s `Rc::get_mut` and falls
through to a fresh frame, preserving snapshot semantics for the escaped
value.

### MATCH frame lifetime under tail recursion

When a user-fn recurses through a `MATCH` arm, the recursive call sits
inside the MATCH-built per-call frame, not the user-fn's own frame.
MATCH clones the user-fn's frame storage Rc onto its own frame's
`FrameStorage.outer`, so the user-fn frame's storage stays alive for the
duration of the arm body — without that chained Rc, the recursive arm
body's `outer` pointer into the dying frame would dangle on TCO Replace.
A reserve whose shell is still aliased fails `try_reset_for_tail`'s
`Rc::get_mut` and falls through to a fresh frame; reuse resumes once the
alias drops.

The bound the `chained_user_fn_tail_calls_reuse_one_slot` and
`match_driven_tail_recursion_completes` tests pin is: the user-fn frame
is alive across exactly one MATCH-arm iteration at a time, and the call
chain collapses to one scheduler slot via the `Tail` rewrite even when a
reset refuses on individual MATCH-arm steps.

## Ping-pong reserve frame

An invoke runs synchronously while the slot's `scope` borrows into the
**active** frame's region, so that frame's tree-borrows protector is live
across the invoke: resetting the active frame in place mid-step would
deallocate the region out from under a live borrow. Tail reuse therefore
never touches the active frame — it draws from a **different** frame, two
iterations old, that is past every live protector.

To supply one, the slot carries a per-iteration **reserve frame** in
`Frame::reserve` that ping-pongs across `NodeStep::Replace`:

- **Replace arm in `execute.rs`.** On a new-frame Replace, drop the
  (now two-iterations-old) reserve, rotate the post-step frame into
  the slot's `reserve`, install the new frame as the slot's `cart`.
  First iteration's reserve stays `None`; second iteration fills it;
  iteration 3+ has a reserve to consume.
- **Reserve-consuming `acquire_tail_frame`.** `with_slot_step` drains
  the slot's `reserve` into `Scheduler::active_reserve`; on the next
  invoke, `acquire_tail_frame` takes it and calls `try_reset_for_tail`.
  Its shell `strong_count` is 1 (only the reserve field held it), so the
  reset lands and the body runs in the reset region. A value that escaped
  while that frame was the active cart two iterations ago pins the
  *storage*, not the shell, so it doesn't foreclose the reset — its
  snapshot rides the old `FrameStorage` while the shell reuses. Only a
  lingering *shell* clone makes `Rc::get_mut` refuse, and
  `acquire_tail_frame` then allocates fresh instead.

The dispatcher reads the slot's reserve / active-frame state from the
execution layer (see [execution/README.md § The dispatcher / scheduler
boundary](../execution/scheduler.md#the-dispatcher--scheduler-boundary)):
`dispatch::exec::invoke` is a pure decide against a `SchedulerView`, and the
harness `apply_outcome` arm acquires the cart via `KoanRuntime::acquire_tail_frame`
before handing it to the decide. The `active_frame` / `active_reserve` state lives
on the driver's ambient context (`KoanRuntime`), not the scheduler — the scheduler
is a pure DAG runtime; the accessor surface is what dispatch sees.

The two-iteration gap is the safety witness: when iteration N consumes
the reserve, the reserve's scope was the active scope on iteration
N-2 and is past every live tree-borrows protector by the time
iteration N's invoke fires. Miri full-slate green on
`recursive_tagged_match_no_uaf` — which exercises exactly this pattern
at every iteration — under `MIRIFLAGS=-Zmiri-tree-borrows` is the
structural confirmation.

Steady-state allocation on the stateful keyworded /
`FunctionValueCall` recursive loop is one `KoanRegion` per iteration
(the inner region `try_reset_for_tail` installs); the `CallFrame`
shell and its `Rc` reuse across iterations after the first
two-iteration warmup.


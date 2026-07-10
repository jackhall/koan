# Frame management

Active-frame propagation and the outer-frame chain for builtin-built frames.
Part of the [per-call region protocol](README.md). Tail-call region turnover —
how a tail hop runs in constant space — is owned by
[tail-call-optimization.md](../tail-call-optimization.md).

## Active-frame propagation

The interpreter exposes the currently running slot's frame to code that needs
to capture it ([builtin-built frame chaining](#outer-frame-chain-for-builtin-built-frames)
below, deferred sub-Dispatch under a per-call frame). The state lives on the
driver's ambient context ([`KoanRuntime`](../../src/machine/execute/ambient.rs)),
not the scheduler — the scheduler is a pure DAG runtime:

- **`active_frame: Option<Rc<CallFrame>>`** — the cart of the slot
  currently being executed. Read through
  [`KoanRuntime::current_frame`](../../src/machine/execute/ambient.rs);
  written only by `KoanRuntime::with_slot_step` (the RAII bracket
  `run_step` wraps each slot step in) and the `KoanRuntime::with_active_frame`
  bracket. An invoke never empties it, so within a step it is always
  `Some` — `PostStep::prev_frame` (the slot's cart at step end) is
  non-optional.
- **`KoanRuntime::with_active_frame(frame, body) -> R`** — brackets
  `frame` as `active_frame` for the duration of `body`, restoring the
  previous one on every exit path, unwind included. Used by
  [`KoanRuntime::dispatch_body`](../../src/machine/execute/runtime/submit.rs) to
  dispatch a body's non-tail statements under the body frame so each sub-slot
  inherits it as its cart (see
  [typing/functors.md § Deferred return-type elaboration](../typing/functors.md#deferred-return-type-elaboration)
  for the per-call type-side bind that motivates it).

`run_step` sources the slot's cart from its scheduler-held anchor
([`SlotFrame.cart`](../../src/machine/execute/nodes.rs)) and installs it as
`active_frame` for the duration of the step via `with_slot_step`. Sub-dispatch
and dep-finish slots inherit `active_frame` so they see the right ancestor for
their own chaining decisions.

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

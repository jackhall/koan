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
whenever the child's `outer` points into per-call memory.

That pin is **derived**, not threaded by the caller. `CallFrame::new`
reads it off the parent scope via
[`Scope::parent_frame_pin`](../../src/machine/core/scope.rs): the parent
scope's own `region_owner` when the parent lives in a per-call region, or
no pin when it lives in the run-root region (which outlives the run — a
root chain plus an escaping value's reach-set pin would close a
`region → value → frame` cycle). There is no pin parameter for a caller
to mis-wire. The TCO fresh-tail cart is minted through the **same**
`CallFrame::new`, with the callee closure's captured definition scope as
its parent, so it chains that scope's region owner exactly like any other
frame: a top-level-defined recursive fn captures the run-root scope and
therefore chains nothing (TCO recursion stays bounded), while a closure
capturing a per-call frame chains it so that frame survives the hop that
retires the caller.

The builtins that build their own per-call frame — MATCH and TRY through
`branch_walk.rs`'s `arm_tail`, EVAL directly:

- `match_case.rs` — MATCH constructs a frame whose child scope's
  `outer` is the **call-site** scope so free names in the arm body
  resolve against the surrounding call.
- `try_with.rs` — TRY-WITH dispatches each branch under a frame
  chained to the TRY call site so the branch body's free names
  resolve through the surrounding call.
- `eval.rs` — EVAL builds a per-call frame for the evaluated
  expression.

(MODULE builds no per-call frame — its declarations are a same-region
child of the call site, so nothing chains.)

Field declaration order on `FrameStorage`
is load-bearing: `region` is declared before `outer`, so the
auto-derived `Drop` tears down this frame's region *before* releasing
the parent storage Rc — inner pointers die before the outer storage they
may reference. The frame shell holds its child scope as a delivery envelope
([`Delivered`](../../workgraph/src/witnessed/delivered.rs)) whose host is the
storage — one co-located carrier, built witnessed at construction. Dropping the
sealed carrier never dereferences the child pointer, so the shell needs no
drop-order rule of its own.

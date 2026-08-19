# Frame management

Active-frame propagation, the lexical-chain reshape a replace installs, and the
outer-frame chain for builtin-built frames.
Part of the [per-call region protocol](README.md). Tail-call region turnover —
how a tail hop runs in constant space, and why no hold spans the hop — is owned by
[tail-call-optimization.md](../tail-call-optimization.md); the slot's stored scope
handle is owned by [scope-handles.md](scope-handles.md).

## Active-frame propagation

The interpreter exposes the currently running slot's frame to code that needs
to capture it ([builtin-built frame chaining](#outer-frame-chain-for-builtin-built-frames)
below, deferred sub-Dispatch under a per-call frame). The state lives on the
driver's ambient context
([`AmbientContext`](../../src/machine/execute/ambient.rs), a field of the koan-side
`Host`), not the scheduler — the scheduler is a pure DAG runtime:

- **`active_frame: Option<Rc<CallFrame>>`** — the cart of the slot
  currently being executed. Read through the ambient context
  ([`ambient.rs`](../../src/machine/execute/ambient.rs));
  written only by `Host::with_slot_step` (the RAII bracket
  `Host::step` wraps each slot step in) and the `Host::with_active_frame`
  bracket. The bracket installs the slot's non-optional cart and an invoke
  never empties it — a `FreshTail` placement mints its own cart rather than
  touching the active one — so within a step it is always `Some`. It stays an
  `Option` because it is legitimately `None` *between* steps: a submission
  arriving outside any step reads exactly that, and `submission_cart` falls
  back to the run frame and reports the new slot unframed.
- **`Host::with_active_frame(frame, body) -> R`** — brackets
  `frame` as `active_frame` for the duration of `body`, restoring the
  previous one on every exit path, unwind included. Used by
  [`Host::dispatch_body`](../../src/machine/execute/harness.rs) to
  dispatch a body's non-tail statements under the body frame so each sub-slot
  inherits it as its cart (see
  [typing/functors.md § Deferred return-type elaboration](../typing/functors.md#deferred-return-type-elaboration)
  for the per-call type-side bind that motivates it).

`Host::step` sources the slot's cart from its scheduler-held anchor
([`SlotFrame.cart`](../../src/machine/execute/nodes.rs)) and installs it as
`active_frame` for the duration of the step via `with_slot_step`. Sub-dispatch
and dep-finish slots inherit `active_frame` so they see the right ancestor for
their own chaining decisions.

## Lexical-chain reshape at the replace

A slot's lexical position is an `Rc<LexicalFrame>` cactus chain stored on its
anchor's `NodePayload` ([`nodes.rs`](../../src/machine/execute/nodes.rs)). The
chain a tail replace installs is decided in one half of the step and assembled in
the other, because the two inputs it needs are never live at the same moment:

- **The decision reads the return contract.** Whether a tail is an FN-body invoke
  or a block entry is a property of the
  [`ReturnContract`](../../src/machine/core/kfunction/body.rs) variant, and the
  contract is live only at the `Outcome::Continue` construction site —
  [`tail_continue`](../../src/machine/execute/outcome.rs) seals it onto the
  replacement continuation, and `Outcome::Continue` carries no contract field of
  its own, so the apply has nothing left to re-read.
- **The assembly reads the frame the body runs in**, which only the apply
  resolves: the cart a `FreshTail` / `FreshChild` placement carries, else the
  slot's current cart for an `Inherit` FN-body re-entry.

[`ChainOp`](../../src/machine/execute/nodes.rs) is what carries the first across to
the second. It names no lifetime — a `ScopeId` and a body index — so it rides the
outcome past the contract's erasure, and `ChainOp::apply` turns it into the
`Rc<LexicalFrame>` the fresh anchor stores. Both ends are lifetime-free, so a node
pins no `'run` through its chain.

Three reshapes:

- **`Unchanged`** — a tail in the same lexical block; the chain rides over
  untouched.
- **`AssembleBody { body_index }`** — an FN-body invoke (a `Function` or `PerCall`
  contract). [`assemble_body_chain`](../../src/machine/core/lexical_frame.rs)
  rebuilds the chain from the body scope's lexical `outer` walk, read through
  `CallFrame::with_scope` against the body frame, so depth tracks source-level
  nesting rather than call depth and a recursive tail chain's stored chain does
  not grow per hop.
- **`PushBlock { scope_id, body_index }`** — a block entry under any other
  contract (a MATCH / TRY arm, a `USING` overlay): prepend one frame, with
  `body_index` positioning it for multi-statement tail-into-last.

The apply keys its fresh-anchor decision on the same variant rather than on
whether a cart was minted
([`harness.rs`](../../src/machine/execute/harness.rs)): an `Inherit` FN-body
re-entry installs no frame yet reshapes the chain, so a frameless replace still
mints a fresh anchor whenever the variant is not `Unchanged`.

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
scope's own region owner — read off its region's host back-link — when the
parent lives in a per-call region, or
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

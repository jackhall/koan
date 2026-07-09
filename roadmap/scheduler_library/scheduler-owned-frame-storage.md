# Scheduler-owned frame storage

**Problem.** [`CallFrame`](../../src/machine/core/arena.rs) strong-owns its
region storage (`Rc<FrameStorage>`) — the one koan-side strong region owner in
the per-call lifecycle — while the scheduler already pins every per-node
cart (the cart: the scheduler's per-slot `Rc<W::Cart>` memory anchor, koan's
`Rc<CallFrame>`) opaquely as `NodeFrame.cart`
([nodes.rs](../../workgraph/src/scheduler/nodes.rs)) and holds frame owners
on its own rows (`retain`, `handoff` in
[dep_graph.rs](../../workgraph/src/scheduler/dep_graph.rs)).
Because the owner lives koan-side, bare frame `Rc`s cross the boundary at
every seam:

- `Scheduler::finalize` takes `host = prev_frame.storage_rc()` — a projection
  of the cart the scheduler already holds — and `Scheduler::replace` takes a
  `retiring` frame the same way
  ([run_loop.rs](../../src/machine/execute/run_loop.rs)).
- The declared-return finalize hook
  ([finalize.rs](../../src/machine/execute/finalize.rs)) receives a bare
  producer frame, extracts `storage_rc()`, seals its own `Delivered` envelope,
  and hand-assembles `FrameSet` pins for the type-channel check — relocation
  mechanism inside a consumer hook.
- The run-step open pin (`combined` in run_loop.rs) is hand-folded from bare
  frame owners — `FrameSet::singleton(cart.storage_rc())` unioned with each
  dep's liveness set and the contract witness — redundantly with the delivery
  envelopes already held across the open.
- `CallFrame::new`'s `outer_frame` parameter is koan asserting which frames
  strong-own which ancestor storage — a link the scheduler's caller graph
  already encodes.

Two `Workload` associated types ride the trait only because of this
ownership placement. `Cart` (`CallFrame`) is the scheduler's per-slot memory
anchor and reattach witness solely because the shell transitively pins the
storage; with the owner scheduler-held, the storage itself is the correct
anchor and the shell is koan data. `Payload` (`NodePayload` — lexical
position and scope shape) has zero dormant readers in workgraph: it is
stored at alloc and moved out at `take_for_run` (the deadlock diagnostic
renders from `NodeWork.carrier`, not the payload), so it is closure-capture
data wearing a trait type
([workcell.md § What is deliberately absent](../../design/workcell.md)).

**Acceptance criteria.**

- The producer frame owner lives in scheduler-held per-slot state (an
  `Rc<W::Frame>` beside the cart); `CallFrame` holds no strong
  `Rc<FrameStorage>` and is a pure semantics shell — scope carrier, return
  contract, close-owner slot.
- `Scheduler::finalize` and `Scheduler::replace` take no frame-`Rc` argument;
  retention and the TCO handoff seed from scheduler-owned state.
- The Done boundary hands the workload finalize hook its terminal already
  sealed as a `Delivered` envelope; the hook supplies the declared-return
  re-stamp fold and calls neither `storage_rc()` nor `Delivered::seal`.
- The run-step open pin is scheduler-supplied or library-derived from what is
  opened; the run loop folds no `FrameSet` from bare `storage_rc()` owners.
- `Workload` carries no `Cart` type: the scheduler's per-slot memory anchor
  is the frame storage (`Rc<W::Frame>`), and the continuation reattach is
  witnessed by it; the per-call shell is koan-side data riding the step.
- `Workload` carries no `Payload` type: the node's lexical position and
  scope shape ride the continuation's captures.

**Directions.**

- *Step-open pin ownership — decided.* The scheduler supplies the step's open
  pin from the per-slot frame state this item introduces (e.g. returned
  alongside the node from the fused `take_for_run`); no standalone koan-side
  open-builder surface. (Contract-audit fork ruling, 2026-07-07.)
- *Ancestor ownership — open.* Whether scheduler-owned frames dissolve
  `CallFrame::new`'s `outer_frame` policy into a graph-derived ancestor link,
  or koan keeps asserting ancestor ownership as a policy input.
- *Envelope minting point — open.* Where the finalize hook's `Delivered` is
  sealed: inside the scheduler's Done boundary (lifecycle) or by the node
  store when the slot's frame is read.
- *Shell placement — open.* Where the demoted `CallFrame` shell rides once
  it pins nothing: a continuation capture, or a field of koan's step
  context.

## Dependencies

**Requires:**


**Unblocks:**

- [Carving the workcell crate](workcell-extraction.md) — the cell contract's
  single memory anchor is the shape this item lands.
- [Publishing the workgraph crate](workgraph-extraction.md) — this item moves
  the boundary; the frozen API is the post-move one.

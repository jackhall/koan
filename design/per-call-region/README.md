# Per-call region protocol

The contract for [`Rc<CallFrame>`](../../src/machine/core/arena.rs): which
[`KObject`](../../src/machine/model/values/kobject.rs) variants carry a
per-call anchor, how
[`lift_kobject`](../../src/machine/execute/lift.rs) decides to attach one,
how the `alloc_object` cycle gate routes self-referential allocations,
how the [scheduler](../../src/machine/execute/run_loop.rs) propagates the
active frame, how builtin-built frames chain the call-site frame's
storage through `FrameStorage.outer`, and how the TCO step reuses the
frame shell over a fresh `FrameStorage`.
The participants live in `KObject` (carriers), `arena.rs` (allocation
/ storage), and `Scheduler` (active-frame plumbing); this page is the
single named owner so a reader investigating the protocol lands here
rather than reconstructing it from five docs and ten source files.


## The protocol, in three parts

- [Region lifecycle: allocation and lift](lifecycle.md) — which carriers anchor a
  per-call region, the lift-time anchor decision, consumer-pull node-output lift,
  and the `alloc_object` cycle gate.
- [Frame management](frames.md) — active-frame propagation and the outer-frame
  chain for builtin-built frames.
- [Scope handles and verification](scope-handles.md) — the slot-table scope
  handle, the seed-side re-anchor, cross-doc context, and the verification slate.

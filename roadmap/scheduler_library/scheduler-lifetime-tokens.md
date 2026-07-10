# Scheduler-owned lifetime tokens

Type the two temporal invariants that today rest on construction doors and
comments: the within-step life of empty-witness carriers, and the frame
`outer`-chain.

**Problem.** Two frame/step invariants are upheld by discipline the compiler
never sees. First, empty-witness carriers
(`RegionBrand::alloc_object_witnessed`, `Witnessed::resident` returns such
as the `done_resident` doors) pin nothing; soundness rests on the active
frame pinning the region for the construction step and finalize folding the
producer before the carrier lands on a node
([arena.rs](../../src/machine/core/arena.rs) doc on
`alloc_object_witnessed`). Safe code that stashes such a carrier past its
step, or returns a foreign-borrowing value through a region-pure marker
path, under-pins with no audit on the path — six sites produce such
carriers today ([print.rs](../../src/builtins/print.rs), `arena.rs` ×3,
[finalize.rs](../../src/machine/execute/finalize.rs),
[interpret.rs](../../src/machine/execute/runtime/interpret.rs)). Second,
`build_frame_child_witnessed` (`arena.rs`) keeps a per-call child's
cross-region parent alive via `FrameStorage`'s `outer` `Rc` chain — an
invariant upheld by one construction door and its callers wiring
`outer_frame` correctly. A caller passing `None` for a per-call parent
under-pins silently; [runtime.rs](../../src/machine/execute/runtime.rs)'s
`FreshTail` placement passes exactly that `None` (sound for the TCO
fresh-tail frame, and indistinguishable from the unsound shape).

**Acceptance criteria.**

- Empty-witness carriers carry a step-scoped brand: stashing one past its
  construction step is a compile error (pinned by a `compile_fail` test),
  and finalize's fold is the only exit to node storage.
- Frame parenting is typed: the no-parent case is a distinct variant
  reserved to the TCO fresh-tail placement, so passing "no parent" for a
  per-call parent is unrepresentable rather than a silent `None`.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Step-brand mechanism — open.* (a) An invariant-lifetime brand minted per
  scheduler step, carried by the empty-witness constructors; (b) a runtime
  step stamp audited at finalize's fold. Recommended: (a) — compile
  enforcement over a late audit.
- *Frame-parent witness — open.* (a) An enum operand
  (per-call parent `Rc` / rooted parent / fresh-tail) replacing
  `Option<Rc<FrameStorage>>`; (b) derive the chain inside `CallFrame::new`
  from the parent scope's own frame storage so callers cannot mis-wire, with
  fresh-tail the one explicit constructor. Recommended: (b) if the parent
  scope can always answer for its storage; else (a).

## Dependencies

Soft ordering: [Witness-derived binding](witness-derived-binding.md) lands
first when both are in flight — fused bind doors shrink the call-site
surface this item retypes.

**Requires:** none — operates on the current veneer layer.

**Unblocks:** none tracked.

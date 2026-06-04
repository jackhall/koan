# Scheduler run/frame lifetime split

Give per-call frame scopes a lifetime distinct from the run lifetime, so the per-frame
borrows the scheduler stores carry their honest (shorter) extent instead of a fabricated
run-length one.

**Problem.** The scheduler threads a single run lifetime `'a` as the universal currency for
everything it touches. [`run_dispatch<'a>`](../../src/machine/execute/dispatch.rs) takes
`scope: &'a Scope<'a>` alongside `expr: KExpression<'a>`, `state: DispatchState<'a>`, and a
`DispatchCtx<'a, '_>`, and returns `NodeStep<'a>`; the
[`BuiltinFn`](../../src/machine/core/kfunction/body.rs) type is
`for<'a> fn(&'a Scope<'a>, &mut dyn SchedulerHandle<'a>, ArgumentBundle<'a>) -> BodyResult<'a>`;
[`SchedulerHandle<'a>`](../../src/machine/core/kfunction/scheduler_handle.rs) and
[`Node<'a>`](../../src/machine/execute/nodes.rs) carry the same `'a`. But a per-call frame's
child scope lives only as long as its [`CallArena`](../../src/machine/core/arena.rs) `Rc` is
held — its arena drops per-frame, the TCO/Done reclamation that keeps loops O(1) memory. The
scheduler papers over the gap by fabricating `'a` for that shorter-lived scope: the unsafe
`anchored_parts` re-anchor and the `Node.scope: &'a` store both stand in for a lifetime the
borrow checker is never shown. The single `'a` is itself load-bearing — nodes store work
built in an earlier step and read each other's outputs across steps
(`read_result -> &'a KObject<'a>`), so it genuinely must span the whole run; per-call scopes
are the one thing nested strictly inside it, and the only thing that needs a shorter one.

**Impact.**

- Per-call scopes carry their real frame lifetime, so the scheduler stores a borrow whose
  extent the borrow checker tracks rather than a fabricated run-length one.
- A branded/yoked frame handle becomes expressible: with a distinct frame lifetime to bind
  to, [Type-enforced frame re-anchor](type-enforced-frame-reanchor.md) can make a re-anchor
  that outlives its frame a compile error and retire its Miri integration pins.
- The dispatch/builtin surface states the scope↦output lifetime relationship in types,
  replacing the arena-drop-order convention that today carries it implicitly.

**Directions.**

- *Mechanism — open.* Two routes. (a) *Split the lifetime:* introduce a second parameter
  `'s` (frame) with `'a: 's`, threaded through `run_dispatch` → `DispatchCtx` →
  `DispatchState`/resume arms → `BuiltinFn` → `BodyResult` → `SchedulerHandle`; outputs that
  borrow from the per-call arena type as `'s`, lifted to `'a` only at the existing Done
  boundary (`lift_kobject`). (b) *De-borrow the graph:* store the dataflow graph as owned
  data (owned/`Rc` work payloads and results) so no run-spanning `'a` is needed and every
  scope borrow is a short reborrow at use — removes the fabrication by removing the long
  lifetime, at the cost of reworking the arena-`&'a` value representation (`KObject<'a>` and
  friends). Recommended: prototype (a) on the dispatch hot path first; (b) reaches further
  into the value model. A home-rolled yoke spike confirmed the abstraction compiles but that
  adoption founders precisely on this weld — feeding a frame-bounded scope into
  `run_dispatch` collapses the whole cascade into a single "scope must outlive `'a`" error,
  which isolated this split as the prerequisite.
- *Scope-handle invariance — decided.* `Scope<'a>` is invariant in `'a`, so a live
  `&'a Scope<'a>` cannot be coerced to a shorter `&'s Scope<'s>` — the obstacle that blocks a
  uniform scope accessor mixing a run-`'a` root scope with a frame-bounded per-call one (a
  `NodeScope::Root(&'a Scope)` arm and a `NodeScope::Yoked(..)` arm fail to share a return
  lifetime). Overcome it with one minimal change, independent of the Mechanism choice above:
  carry the run-root scope through the *same* yoked handle the per-call scopes use, with the
  existing `&'a Scope<'a>` as its cart — a shared reference is already a stable-deref owner, so
  this needs no new allocation. Then every scope is produced by the handle's `get`, which is a
  layout-identity reprojection from the erased `'static` form that the co-located cart proves
  sound — not a coercion of a live reference — so invariance never enters and both accessor
  arms share one frame lifetime. This clears the invariance wall on its own; the run-`'a` weld
  on dispatch and output (the Mechanism bullet) stays the separate, larger half.

## Dependencies

**Requires:** none — foundational engine-internal rework of the scheduler's lifetime model.

**Unblocks:** [Type-enforced frame re-anchor](type-enforced-frame-reanchor.md) — supplies the
distinct frame lifetime that a compile-time re-anchor brand binds to.

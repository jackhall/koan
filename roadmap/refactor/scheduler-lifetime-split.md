# Scheduler run/frame lifetime split

Store per-call frame scopes at their honest (frame-bounded) extent instead of a fabricated
run-length one, so the borrow the scheduler keeps for a frame scope no longer claims a
lifetime the borrow checker is never shown.

**Problem.** The scheduler threads a single run lifetime `'a` as the universal currency for
everything it touches, including per-call frame scopes that live only as long as their
[`CallArena`](../../src/machine/core/arena.rs) `Rc` is held — the arena drops per-frame, the
TCO/Done reclamation that keeps loops O(1) memory. The scheduler papers over the gap by
fabricating `'a` for that shorter-lived scope at two places: the unsafe
`CallArena::anchored_parts` re-anchor mints a fresh `&'a Scope<'a>` at each per-call seed, and
[`Node<'a>`](../../src/machine/execute/nodes.rs) stores that scope as `scope: &'a Scope<'a>`,
held across scheduler steps. Both stand in for the frame's true, shorter extent. The
fabrication is scattered: four production sites birth or re-derive a frame-bounded scope via
`anchored_parts` — the FN-body, MATCH-arm, and TRY-arm seeds
([`invoke.rs`](../../src/machine/core/kfunction/invoke.rs),
[`match_case.rs`](../../src/builtins/match_case.rs),
[`try_with.rs`](../../src/builtins/try_with.rs)), and the tail sink
[`reinstall_with_frame`](../../src/machine/execute/scheduler/node_store.rs) every
`BodyResult::Tail` funnels through. The run `'a` is itself load-bearing — nodes store work
built in an earlier step and read each other's outputs across steps
(`read_result -> &'a KObject<'a>`), so it genuinely must span the whole run; per-call scopes
are the one thing nested strictly inside it, and the only thing that needs a shorter one.

**Acceptance criteria.**

- A per-call frame scope is stored on its slot as a handle bounded by the frame's
  `Rc<CallArena>`, not as a `&'a Scope<'a>`; `Node.scope` no longer claims the run lifetime
  for a frame scope.
- The unsafe `CallArena::anchored_parts` re-anchor is removed — the three seeds and the
  [`reinstall_with_frame`](../../src/machine/execute/scheduler/node_store.rs) sink derive the
  frame scope from the frame cart instead.
- The handle adds no `Rc<CallArena>` clone beyond the slot's existing `Node.frame` and no
  per-read indirection beyond today's single deref, so TCO frame reuse
  (`try_reset_for_tail`'s `strong_count == 1` check) is unaffected.
- `recursive_eval_no_uaf` runs green under `MIRIFLAGS=-Zmiri-tree-borrows` and is admitted to
  [`observe/miri_slate.md`](../../observe/miri_slate.md) — with the stored borrow carrying its
  honest frame extent, the over-wide borrow tree borrows rejects is no longer expressible.

**Directions.**

- *Mechanism — decided.* Store the frame scope as a `NodeScope<'a>` handle on the slot: a
  `Yoked` arm (a yoke over the frame's own `Rc<CallArena>`, with an `&self`-bounded `get`) and
  a `Root(&'a Scope<'a>)` arm for run-root scopes. **Single-cart:** a yoked scope exists iff
  the slot's [`Node.frame`](../../src/machine/execute/nodes.rs) is `Some`, so the `Yoked` arm
  carries no payload of its own and projects the scope through `Node.frame` — no duplicate
  `Rc`, no refcount traffic, no contention with `try_reset_for_tail`'s uniqueness check. The
  projection (`CallArena::scope`, an existing `&self`-bounded re-attach) replaces the
  `anchored_parts` fabrication at all four sites.
- *Handle migration — decided.* The storing [`SchedulerHandle`](../../src/machine/core/kfunction/scheduler_handle.rs)
  methods the seeds use (`add_dispatch_with_chain`, `add_combine`) gain `NodeScope`-taking
  siblings; the existing `&'a Scope` methods become `Root`-wrapping default delegates, so the
  ~55 run-scope call sites are untouched and the trait stays object-safe (an `impl Into<…>`
  parameter would break `dyn SchedulerHandle`).
- *Scope-handle invariance — decided.* `Scope<'a>` is invariant in `'a`, so a live
  `&'a Scope<'a>` cannot coerce to a shorter borrow. The `NodeScope` handle stays invariant in
  `'a` via its `Root` arm, and the run-root scope rides the *same* handle (its live
  `&'a Scope<'a>` as cart — a shared reference is already a stable-deref owner, no allocation),
  so one accessor serves both arms: each `get` is a layout-identity reprojection from the
  erased `'static` form the co-located cart proves sound, never a coercion of a live reference,
  so invariance never enters.
- *Seed set — decided.* The frame-bounded-scope births are exactly the three `anchored_parts`
  seeds plus the tail sink. [`module_def.rs`](../../src/builtins/module_def.rs) is **not** one:
  its `child_scope` is allocated in the parent/run arena (lifetime `'a`), and its only frame
  capture is an `Rc<CallArena>` *anchor* stamped into a `KType::Module` identity — a re-anchor
  the [Type-enforced frame re-anchor](type-enforced-frame-reanchor.md) brand owns, not a frame
  scope this item stores.
- *Phasing — decided.* One small commit per site, each green and Miri-clean, never more than
  one site at a time: (1) substrate — add `NodeScope` and switch `Node.scope` to it, all arms
  `Root`, a provable no-op; (2) the `*_ns` handle siblings, no call-site change; (3) flip the
  `reinstall_with_frame` sink to `Yoked`; (4–6) flip the MATCH, TRY, then FN seeds (simplest to
  most complex); (7) delete `anchored_parts` once unused. Never flip a seed before the
  substrate lands.
- *Within-step read / `'s` on the dispatch surface — deferred.* This item keeps
  `run_dispatch` / `BuiltinFn` at `&'a`, so the read boundary still transiently widens the
  frame-bounded projection to `&'a` to feed them — one concentrated, synchronous-step-scoped
  overstatement, kept sound by the existing `lift_kobject` Done-boundary re-anchor. Threading a
  real `'s` (`'a: 's`) through `run_dispatch → DispatchCtx → BuiltinFn → BodyResult →
  SchedulerHandle` so the borrow checker tracks the frame lifetime *within* a step — the
  ~100-site `BuiltinFn`/trait weld — is deferred to a later item; pursue only if the
  concentrated transient widen proves insufficient.

## Dependencies

**Requires:** none — the frame-bounded scope this stores honestly is born and reclaimed
entirely inside the scheduler's per-call machinery; no other item gates it.

**Unblocks:**

- [Type-enforced frame re-anchor](type-enforced-frame-reanchor.md) — supplies the
  frame-bounded scope handle a compile-time re-anchor brand binds to.

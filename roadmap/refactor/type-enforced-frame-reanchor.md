# Type-enforced frame re-anchor

Thread a distinct per-call frame lifetime through dispatch and lift the frame's borrowed
parts into a brand, so the scheduler re-anchors them by type rather than by hand-maintained
`unsafe` discipline.

**Problem.** A per-call frame's child scope is now *stored* honestly — a payload-less
`NodeScope::Yoked` marker re-projected from the slot's own `Rc<CallArena>` cart (see
[design/per-call-arena-protocol.md § Slot-table scope handle](../../design/per-call-arena-protocol.md#slot-table-scope-handle)) —
and the slot-storage `&'a` fabrication is concentrated to two audited core callers:
[`CallArena::with_anchored_child`](../../src/machine/core/arena.rs) (the seed-side bind of
`it` / FN parameters) and `NodeScope::project` (the read boundary). But both still call the
`unsafe` [`CallArena::anchored_parts`](../../src/machine/core/arena.rs), which fabricates a
free `'a` unconstrained by the `Rc<CallArena>` witness — because `run_dispatch` / `BuiltinFn` /
`SchedulerHandle` still demand `scope: &'a` shared with the work payload and the produced
output, so the read boundary must widen the frame-bounded scope back to the run `'a` to feed
them. With no distinct frame lifetime, no branded handle can make a too-long re-anchor a
compile error: `Scope<'a>` invariance blocks unifying a frame-bounded scope with a run-`'a`
one. The using code stays correct only by convention — `with_anchored_child`'s re-anchor and
the MODULE-body `Combine` continuation that captures the child scope across a scheduler
park/wake boundary. A regression — re-anchoring longer than the frame's `Rc<CallArena>`
witness, or letting a continuation outlive its frame — compiles fine and surfaces only at
runtime under tree borrows. The Miri slate pins those paths with integration tests
(`type_op_dispatch_does_not_dangle`, `try_inside_tco_position_preserves_frame_chain`,
`module_body_dispatch_does_not_dangle`) standing in for a guarantee the compiler does not yet
make, and a separate pre-existing UB, `recursive_eval_no_uaf`, trips tree borrows at the
Done-boundary lift reached through that retained `&'a` widen.

**Acceptance criteria.**

- A distinct per-call frame lifetime `'s` (`'a: 's`) is threaded through `run_dispatch` →
  `DispatchCtx` → `BuiltinFn` → `BodyResult` → `SchedulerHandle`, so a frame-bounded scope
  feeds dispatch and lifts to `'a` only at the `lift_kobject` Done boundary — the read boundary
  no longer widens a frame scope to the run `'a`.
- The `unsafe` [`CallArena::anchored_parts`](../../src/machine/core/arena.rs) re-anchor has no
  remaining caller and is deleted, replaced by a branded frame handle whose `get` hands back
  `'s`; re-anchoring a frame's borrowed parts with a lifetime longer than its `Rc<CallArena>`
  witness is a compile error.
- The MODULE `Combine` continuation threads the captured child scope through the branded frame
  handle, so a continuation that outlives its frame is a compile error.
- The branded handle carries `Scope<'a>`'s invariance structurally and is variance-checked, so
  a covariant coercion that reintroduces a use-after-free is a compile error.
- `recursive_eval_no_uaf` runs green under `MIRIFLAGS=-Zmiri-tree-borrows` and is admitted to
  [`observe/miri_slate.md`](../../observe/miri_slate.md); the integration tests
  `type_op_dispatch_does_not_dangle`, `try_inside_tco_position_preserves_frame_chain`, and
  `module_body_dispatch_does_not_dangle` are removed, each retired only once its regression is
  a *compile* error.
- `opaque_ascription_re_binds_do_not_alias_unsoundly` remains on the Miri slate, since it pins
  a `RefCell`-under-`&Module` borrow discipline rather than a lifetime fabrication.

**Directions.**

- *Thread the frame lifetime `'s` — decided.* Introduce a second lifetime `'s` (`'a: 's`)
  through `run_dispatch` → `DispatchCtx` → `BuiltinFn` → `BodyResult` → `SchedulerHandle`,
  instantiable as `'a` at every seam so it lands inertly first (signature-only, no behavior
  change), then flip the `NodeScope::project` read boundary to hand back `'s` and lift to `'a`
  at the existing `lift_kobject` Done boundary. The `BuiltinFn` fn-pointer alias and the
  `SchedulerHandle` trait are the viral chokepoints (~100 call sites); land the inert widening
  one layer at a time, green and Miri-clean per step, before flipping the read boundary. A
  home-rolled yoke spike confirmed the brand compiles but that adoption founders precisely on
  this weld — feeding a frame-bounded scope into `run_dispatch` collapses the cascade into a
  single "scope must outlive `'a`" error — which isolated the weld as the prerequisite.
- *Brand mechanism for `anchored_parts` — decided.* Once `'s` exists, the brand is a yoke over
  the frame's `Rc<CallArena>` cart whose `get` hands back `'s` — co-locating owner and borrow
  so a re-anchor outliving its frame fails to compile. Both surviving `anchored_parts` callers
  (`with_anchored_child`, `NodeScope::project`) route through it; `anchored_parts` is then
  deleted. Mirror the shipped [`ScopePtr`](../../src/machine/core/scope_ptr.rs) discipline:
  one audited `'static`↔`'s` transmute concentrated in the brand, discharged by the co-located
  cart.
- *Scheduler continuation storage — decided.* The MODULE `Combine` continuation captures a
  per-call child scope across a park for the same root cause; it threads the same branded frame
  handle once `'s` lands, and stays runtime-checked (`module_body_dispatch_does_not_dangle`)
  until then.
- *Out-of-scope tests — decided.* `opaque_ascription_re_binds_do_not_alias_unsoundly` pins a
  `RefCell`-under-`&Module` borrow discipline, not a lifetime fabrication, so it stays an
  irreducibly-dynamic slate pin and is not retired by this item.
- *Variance preservation — decided.* `Scope<'a>` is invariant; as with the shipped `ScopePtr`
  brand, the brand must carry that invariance structurally and must be variance-checked, not
  assumed — covariance silently reintroduces a use-after-free.
- *Validation — decided.* Re-run the full Miri slate before and after via the `miri` skill, and
  retire each integration test only once its regression is a *compile* error — not merely
  "currently green". A passing integration test exercises the adversarial re-anchor shape only
  incidentally, so green is not evidence the type now guards it.

## Dependencies

**Requires:** none — builds on the shipped honest `NodeScope` slot storage (see
[design/per-call-arena-protocol.md § Slot-table scope handle](../../design/per-call-arena-protocol.md#slot-table-scope-handle));
the frame lifetime this threads is born and reclaimed inside the scheduler's per-call
machinery, so no other item gates it.

**Unblocks:** none tracked yet.

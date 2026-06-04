# Type-enforced frame re-anchor

Lift the per-call frame's borrowed parts into a brand so dispatch and the scheduler
re-anchor them by type, not by hand-maintained `unsafe` discipline.

**Problem.** [`CallArena::anchored_parts`](../../src/machine/core/arena.rs) is an
`unsafe` re-anchor that fabricates the slot-storage `'a` for the per-call frame's
`(inner_arena, child): (&'a RuntimeArena, &'a Scope<'a>)` parts. The branded
[`ScopePtr`](../../src/machine/core/scope_ptr.rs) re-attach already concentrates
captured/defining-scope lifetime fabrication at the non-generic `CallArena` boundary
(see [design/memory-model.md § Arena lifetime erasure](../../design/memory-model.md#arena-lifetime-erasure));
`anchored_parts` is the one re-anchor on that boundary still left unsafe, because the
frame's borrowed parts have no brand tying their `'a` to the `Rc<CallArena>` witness.
The using code keeps the re-anchor correct only by convention: `KFunction::invoke`'s
per-call frame re-anchor, the MATCH / TRY-WITH per-branch frames that thread the
`outer_frame` chain across a TCO replace, and the MODULE-body `Combine` continuation
that captures the child scope across a scheduler park/wake boundary. A regression in
any of those — re-anchoring with a lifetime longer than the frame's `Rc<CallArena>`
witness, or letting a continuation outlive its frame — compiles fine and surfaces only
at runtime under tree borrows. The Miri slate pins those paths with integration tests
(`type_op_dispatch_does_not_dangle`, `try_inside_tco_position_preserves_frame_chain`,
`module_body_dispatch_does_not_dangle`) standing in for a guarantee the compiler does
not yet make.

**Impact.**

- Re-anchoring a per-call frame's borrowed parts with a lifetime longer than its
  `Rc<CallArena>` witness is a compile error, so dispatch's frame re-anchor is sound by
  type rather than re-argued in a SAFETY comment at each call.
- The scheduler threads the captured child scope through a branded handle, so a
  continuation that outlives its frame fails to compile.
- The dispatch / scheduler integration tests (`type_op_dispatch_does_not_dangle`,
  `try_inside_tco_position_preserves_frame_chain`, `module_body_dispatch_does_not_dangle`)
  retire — their invariant is now type-enforced — shrinking the Miri slate toward the
  minimal mirrors of the irreducible transmute plus the genuinely-dynamic checks (the
  cycle gate, leak detection, and the `RefCell`-under-`&Module` discipline
  `opaque_ascription_re_binds_do_not_alias_unsoundly` pins).

**Directions.**

- *Brand mechanism for `anchored_parts` — decided.* A home-rolled yoke spike established
  that no branded handle makes this re-anchor a compile error while the scheduler welds the
  scope lifetime to the run `'a`: `Scope<'a>` invariance blocks unifying a frame-bounded
  scope with a run-`'a` one, and `run_dispatch` / `BuiltinFn` / `SchedulerHandle` all demand
  `scope: &'a` shared with the work payload and the produced output. So the branded frame
  handle rides on [Scheduler run/frame lifetime split](scheduler-lifetime-split.md); once a
  distinct frame lifetime exists, the brand is a yoke over the frame's `Rc<CallArena>` cart
  whose `get` hands back that frame lifetime — co-locating owner and borrow so a re-anchor
  outliving its frame fails to compile.
- *Scheduler continuation storage — decided.* The MODULE `Combine` continuation captures a
  per-call child scope at `'a` for the same root cause; it threads the same branded frame
  handle once the lifetime split lands, and stays runtime-checked
  (`module_body_dispatch_does_not_dangle`) until then.
- *Out-of-scope tests — decided.* `opaque_ascription_re_binds_do_not_alias_unsoundly`
  pins a `RefCell`-under-`&Module` borrow discipline, not a lifetime fabrication, so it
  stays an irreducibly-dynamic slate pin and is not retired by this item.
- *Variance preservation — decided.* `Scope<'a>` is invariant; as with the shipped
  `ScopePtr` brand, the brand must carry that invariance structurally and must be
  variance-checked, not assumed — covariance silently reintroduces a use-after-free.
- *Validation — decided.* Re-run the full Miri slate before and after via the `miri`
  skill, and retire each integration test only once its regression is a *compile* error
  — not merely "currently green". A passing integration test exercises the adversarial
  re-anchor shape only incidentally, so green is not evidence the type now guards it.

## Dependencies

**Requires:** [Scheduler run/frame lifetime split](scheduler-lifetime-split.md) — a branded
frame handle can make the re-anchor a compile error only once per-call scopes carry a
lifetime distinct from the run `'a` for the brand to bind to; a spike established the brand
is unreachable while the two are welded. The `CallArena` brand boundary this builds on has
already shipped (the branded [`ScopePtr`](../../src/machine/core/scope_ptr.rs) concentrating
scope-re-attach fabrication at the non-generic `CallArena`); `anchored_parts` is precisely
the unsafe surface that boundary leaves for this follow-up.

**Unblocks:** none tracked yet.

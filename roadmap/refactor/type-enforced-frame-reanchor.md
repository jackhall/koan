# Type-enforced frame re-anchor

Lift the per-call frame's borrowed parts into a brand so dispatch and the scheduler
re-anchor them by type, not by hand-maintained `unsafe` discipline.

**Problem.** [`CallArena::anchored_parts`](../../src/machine/core/arena.rs) is an
`unsafe` re-anchor that fabricates the slot-storage `'a` for the per-call frame's
`(inner_arena, child): (&'a RuntimeArena, &'a Scope<'a>)` parts — and the
[type-enforced scope re-attach](type-enforced-scope-reattach.md) item deliberately
leaves it unsafe, concentrating fabrication at `CallArena` rather than removing it.
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

- *Brand mechanism for `anchored_parts` — open.* `anchored_parts` returns
  `(&'a RuntimeArena, &'a Scope<'a>)` from a non-generic `CallArena` (it backs
  `Rc<CallArena>`, which carries no lifetime), so it hits the same no-`'a`-to-brand
  obstacle as the sibling re-attach item: the brand must tie the returned `'a` to the
  `Rc<CallArena>` witness. Options: (a) a branded handle minted from the `Rc` whose
  lifetime cannot outlive it; (b) keep `anchored_parts` as the single documented unsafe
  boundary and brand only the downstream threading. Recommended: settle the sibling
  item's `CallArena` brand first, then extend the same brand here.
- *Scheduler continuation storage — open.* The `Combine` continuation stores the
  captured child scope through the lifetime-erased scheduler node path; for
  `module_body_dispatch_does_not_dangle` to retire, that stored handle must carry a
  brand that survives the park/wake boundary, or the path stays runtime-checked.
  Determine whether scheduler node storage can hold a branded scope without
  reintroducing a fabrication.
- *Out-of-scope tests — decided.* `opaque_ascription_re_binds_do_not_alias_unsoundly`
  pins a `RefCell`-under-`&Module` borrow discipline, not a lifetime fabrication, so it
  stays an irreducibly-dynamic slate pin and is not retired by this item.
- *Variance preservation — decided.* `Scope<'a>` is invariant; as with the sibling
  item, the brand must carry that invariance structurally and must be variance-checked,
  not assumed — covariance silently reintroduces a use-after-free.
- *Validation — decided.* Re-run the full Miri slate before and after via the `miri`
  skill, and retire each integration test only once its regression is a *compile* error
  — not merely "currently green". A passing integration test exercises the adversarial
  re-anchor shape only incidentally, so green is not evidence the type now guards it.

## Dependencies

**Requires:** [Type-enforced scope re-attach](type-enforced-scope-reattach.md) — builds
on that item's `CallArena` brand boundary and the concentration of lifetime fabrication
there; `anchored_parts` is precisely the unsafe surface that item leaves for follow-up.

**Unblocks:** none tracked yet.

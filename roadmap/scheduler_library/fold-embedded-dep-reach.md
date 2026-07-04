# Fold embedded dep reach at the finish surface

Guarantee 5 of [design/scheduler-library.md](../../design/scheduler-library.md)
— embedding a dep in an output folds the dep's reach into the output's reach
set by construction — enforced at every finish that embeds a dep-derived value.

**Problem.** The field-list dep finishes
([field_list.rs](../../src/machine/execute/dispatch/field_list.rs),
`defer_field_list` / `defer_field_list_action`) read each sub-dispatch
terminal's un-relocated `t.value`, clone the `KType` into the folded field
list, and seal the result through `alloc_carried` — own-region-only reach.
The dep's `t.carrier`, which names the regions the type's borrows reach, is
never folded into the result. A `KType` clone can carry internal borrows
(`KType::KFunctor { body: Option<&'a KFunction<'a>> }`), and
`RegionBrand::alloc_ktype` accepts `KType<'_>` at any lifetime (a
lifetime-erasing store), so the clone smuggles a foreign borrow into a
carrier whose reach omits the borrowed region: a record field typed by a
bound functor whose defining frame dies while the sealed record-type carrier
is still alive reads freed memory through `body`. Meanwhile the library
combinator built to make this unrepresentable —
[`StepContext::alloc_with`](../../workgraph/src/witnessed/step_ctx.rs) — has
zero consumers in the repo ([arena.rs](../../src/machine/core/arena.rs) ~402
says so), the [attr.rs](../../src/builtins/attr.rs) module-member re-tag
hand-rolls the same fold with raw `yoke_branded` + `merge` currency inside a
builtin, and the one-type-carrier compound
`alloc_carried(|b| Carried::Type(b.alloc_ktype(...)))` is copy-pasted at
roughly twelve call sites, so a fix to how a type carrier is born is twelve
edits.

**Acceptance criteria.**

- Every finish that embeds a dep-derived value seals its result with a reach
  set covering the own region unioned with each embedded dep's reach; the
  field-list twins fold `t.carrier` for every field type they clone in.
- A koan-side `alloc_carried_with` (sibling of `alloc_carried` on
  `KoanStepContextExt`) wraps `StepContext::alloc_with`, and the
  [attr.rs](../../src/builtins/attr.rs) module-member re-tag routes through
  it — no raw `yoke_branded`/`merge` construction remains in builtins.
- An `alloc_type` helper owns the one-`KType`-carrier construction, and the
  hand-rolled `alloc_carried(|b| Carried::Type(b.alloc_ktype(...)))` sites
  call it.
- The sibling sites whose `kt` derives from a dep terminal
  ([val_decl.rs](../../src/builtins/val_decl.rs),
  [parameterized_types.rs](../../src/builtins/parameterized_types.rs), the
  `expect_type_result` consumers of
  [resolve_or_await.rs](../../src/builtins/resolve_or_await.rs)) are audited
  against the same criterion, each either folding its dep's reach or carrying
  a comment stating why its value is region-pure.
- A Miri test exercises a functor-typed field whose defining frame drops
  while the folded record-type carrier is still held, and the slate stays at
  0 UB / 0 leaks.

**Directions.**

- Fold mechanism — open. (a) Route the finalize through
  `StepContext::alloc_with` via the `alloc_carried_with` wrapper, so the fold
  is the call shape itself (guarantee 5 by construction, and the library
  combinator gains its first consumer); (b) union the dep witnesses into the
  `Witnessed` before sealing (smaller diff, but the fold stays caller-side
  discipline). Recommended: (a).
- `alloc_type` placement — decided. Sibling of `alloc_carried` on
  `KoanStepContextExt` in [arena.rs](../../src/machine/core/arena.rs).
- Narrowing `alloc_ktype`'s any-lifetime signature so a borrow-carrying type
  cannot cross a brand unwitnessed at compile time — deferred. A separate
  compile-enforcement spike; this item closes the reach hole at the finish
  surface without changing the store's signature.

## Dependencies

**Requires:** none — the step-construction-context substrate this builds on
is shipped.

**Unblocks:**
- [Carrier-only catch delivery](catch-carrier-delivery.md) — the fold
  discipline this item establishes is the pattern the catch finish adopts.

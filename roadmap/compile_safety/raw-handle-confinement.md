# Raw-handle confinement

Confine raw `RegionHandle` access to `machine::core`, so the veneer is the only
door koan code stores through.

**Problem.** `pub(crate)` `RegionBrand::handle()`
([arena.rs](../../src/machine/core/arena.rs)) hands any koan crate code the raw
`RegionHandle`, bypassing every veneer door; production reaches exist
across `src/machine`, most inside `machine::core` and several outside
(`execute/decide/constructors.rs`, `execute/decide/literal.rs`,
`model/values/kobject.rs`). Most routes the raw handle exposes are vetted by
their own signature — the `!needs_drop`-asserted `in_place` / `frozen_table`
verbs, the rank-2 `bump_born_with` door, and the `mint_retained` composition —
but the `Copy`-bounded bump primitives (`value` / `slice`) vet drop glue only:
nothing in their signature mints a reach for a region-borrowing `Copy` value,
so a foreign-branded store there is held off by lifetime discipline, not by a
reach. Confining the brand door alone does not type the reach shut: workgraph
publishes `RegionHandle::from_owner` and `FoldedPlacement::handle()` as `pub`,
so any koan module holding an `Rc<FrameStorage>` or a placement can mint a raw
handle without it.

**Acceptance criteria.**

- No production call site outside `machine::core` obtains a raw
  `RegionHandle` — by any route (`RegionBrand::handle()`,
  `RegionHandle::from_owner`, `FoldedPlacement::handle()`).
- `RegionBrand::handle()` is not visible outside `machine::core`.
- The outside-core work the raw handle served routes through veneer doors
  whose signatures carry the vetting: the shared destination-operand door, the
  operand-seed constructors, a `FoldingBrand`-fronted section build, and a
  `RegionBrand`-level host accessor.
- Production koan code cannot reach the workgraph raw mints
  (`RegionHandle::from_owner`, `FoldedPlacement::handle()`); tests reach a
  handle only through a `test-hooks`-gated accessor.
- The confinement adds no koan-side `unsafe impl`: `src/`'s production code
  still carries no `unsafe` at all.

**Directions.**

- *Outside-core replacement — decided.* Promote `Scope::dest_operand`
  ([reach.rs](../../src/machine/core/scope/reach.rs)) to `pub(crate)` — three
  of the seven sites duplicate it verbatim — and add veneer doors for the
  rest. The alternative (koan-side `RegionBrand`-headed witness families over
  a local `unsafe impl HasRegionHandle`) is rejected: it puts `unsafe` into
  koan's production code.
- *Workgraph-side mints — open.* (a) Narrow `RegionHandle::from_owner` and
  `FoldedPlacement::handle()` to `pub(crate)` with a `test-hooks` accessor;
  (b) keep them `pub` for embedders and gate koan's use by module discipline
  plus a lint. Recommended: (a) — (b) leaves the reach open by construction,
  and the published-surface cost falls on workgraph's own embedder story.

## Dependencies

**Requires:** none — visibility and veneer work over existing doors.

**Unblocks:** none tracked.

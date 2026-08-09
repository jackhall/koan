# Bump verbs collapse onto the allocator handle

**Problem.** The same three `Copy`-guarded bump verbs are defined at four
surfaces: `Region::bump_text` / `bump_slice` / `bump_value`
([workgraph/src/witnessed/region.rs](../../workgraph/src/witnessed/region.rs)),
their `RegionHandle` re-exposures, `BumpPlacement::text` / `slice` / `value`
([workgraph/src/witnessed/bump.rs](../../workgraph/src/witnessed/bump.rs)),
and koan's `RegionBrand::alloc_text` / `alloc_slice` / `alloc_value`
([src/machine/core/arena.rs](../../src/machine/core/arena.rs)). The frozen
`BumpMap` door is duplicated the same way (`Region::bump_map`,
`RegionHandle::bump_map`, `RegionBrand::alloc_map`). Every layer restates the
same `T: Copy` bounds and the same doc rationale, and each addition to the verb
set multiplies across all four. `BumpAllocator` (shipped by the binding-tables
item) is the natural single home: a `Copy`, brand-confined handle every one of
those surfaces can mint.

**Acceptance criteria.**

- The `text` / `slice` / `value` verbs are defined once, as methods on
  `BumpAllocator<'b>`, carrying the `T: Copy` bounds that make forgoing the
  destructor lossless.
- `RegionHandle`, `BumpPlacement`, and `RegionBrand` each expose an
  `allocator()` accessor and no per-verb copies; `Region`'s bump verbs are
  crate-internal at most.
- `BumpMap` and `alloc_map` are deleted: their users build hashbrown tables
  over `BumpAllocator` directly, frozen by usage, with the no-drop-glue
  compile-time assert at each declaration site.
- The raw `Allocator` trait impl remains reachable only for collection
  construction; embedder value allocation goes through the guarded verbs.
- koan's typed doors (`alloc_scalar`, `alloc_string`, the witnessed and folded
  variants) are unchanged — they mint carriers, not raw bytes, and are out of
  scope.

**Directions.**

- Verb home — decided per the fork discussion recorded in
  [bump-backed-bindings](bump-backed-bindings.md) planning: methods on
  `BumpAllocator` with `T: Copy` bounds; the trait impl itself cannot express
  the Drop-free guard, so the verbs move rather than disappear.
- Frozen-table replacement — open. Either former `BumpMap` users hold plain
  hashbrown maps with a shared const-assert construction helper (recommended:
  no wrapper to maintain), or a thin read-only veneer preserves the
  frozen-at-construction surface as a type.

## Dependencies

**Requires:**

- [Bump-backed binding tables](bump-backed-bindings.md) — ships
  `BumpAllocator` and the no-drop-glue assert discipline this item spreads.

**Unblocks:** none — leaf.

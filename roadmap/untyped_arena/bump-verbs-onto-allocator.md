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
set multiplies across all four. `BumpAllocator`
([workgraph/src/witnessed/bump.rs](../../workgraph/src/witnessed/bump.rs)) is the
natural single home: a `Copy`, brand-confined handle every one of those surfaces
can mint, already carrying the region's brand as its own lifetime.

**Acceptance criteria.**

- The `text` / `slice` / `value` verbs are defined once, as methods on
  `BumpAllocator<'b>`, carrying the `T: Copy` bounds that make forgoing the
  destructor lossless.
- `RegionHandle`, `FoldedPlacement`, and `RegionBrand` each expose an
  `allocator()` accessor and no per-verb copies; `Region`'s bump verbs are
  crate-internal at most.
- `BumpPlacement` is deleted: `fold_and_bump`'s construct closure receives the
  `BumpAllocator<'b>` directly. The rank-2 fold brand is what confines a
  fold's write surface; the type's mint privacy guarded only the verbs that
  now live on the allocator every handle-holder can mint.
- `BumpMap` and `alloc_map` are deleted: their users build hashbrown tables
  over `BumpAllocator` directly, frozen by usage, with the no-drop-glue
  compile-time assert at each declaration site.
- The raw `Allocator` trait impl remains reachable only for collection
  construction; embedder value allocation goes through the guarded verbs.
- koan's typed doors (`alloc_scalar`, `alloc_string`, the witnessed and folded
  variants) are unchanged — they mint carriers, not raw bytes, and are out of
  scope.

**Directions.**

- Verb home — decided. Methods on `BumpAllocator` with `T: Copy` bounds; the
  trait impl itself cannot express the Drop-free guard, so the verbs move rather
  than disappear.
- Frozen-table replacement — decided. Former `BumpMap` users hold plain
  hashbrown maps built through one shared koan-side `frozen_table` helper
  carrying the element-type no-drop-glue const assert; no veneer type. The
  header is placed through a Copy-relaxed `BumpAllocator::place` verb guarded
  by a monomorphization-time `!needs_drop` assert, which a `ManuallyDrop`
  wrapper (deref'd away before any holder sees it) satisfies — the same
  reasoned admission `Bindings`' bucket vec already makes.

## Dependencies

**Requires:** none — the `BumpAllocator` seam the verbs collapse onto is shipped,
alongside the no-drop-glue assert discipline this item spreads
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)).

**Unblocks:** none — leaf.

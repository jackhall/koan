# Scopes move into the region bump

Retires the `typed-arena` dependency and workgraph's typed-storage machinery by
making `Scope` a `Drop`-free resident of the region bump — the storage every
other family already uses
([design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).

**Problem.** `Scope` is the last typed family
(`KoanStorageProfile::Families = (Scope<'static>, ())`,
[src/machine/core/arena.rs](../../src/machine/core/arena.rs)). Hosting one
family in a typed cell forces workgraph to carry the whole typed-storage
machinery — `FamilyArena`, `Stored`, `FamilyList` / `StorageOf`,
`StorageProfile::Families`, `alloc_resident`'s erase path — and forces every
scope store through `erase_to_static` plus the `Scope<'static>` ⇄ `Scope<'a>`
retype. The bump needs none of that: the allocator is lifetime-free, so a
bumped value is born at `'a` and holds `&'a` into its own region with no
erasure and no retype
([workgraph/src/witnessed/region.rs](../../workgraph/src/witnessed/region.rs)).
What keeps `Scope` out of the bump is its remaining `Drop`: the per-scope
`region_owner: Weak<FrameStorage>` field — a per-scope copy of the fact the
region already stores as its `host` back-link — the root-only `Box<dyn Write>`
writer, and the `ScopeKind` payloads, which carry an owned `String` name and, on
the SIG arm, a bump-backed slot collector whose own map destructor is left to run
even though it frees nothing. The binding tables themselves are already
`Drop`-free — every table is a `hashbrown` map over the region's `BumpAllocator`
with bumped keys and bumped entry payloads, and `Bindings` asserts at compile
time that it contributes no glue at all
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)).

**Acceptance criteria.**

- `Scope` is allocated into its region's bump at the caller's `'a` directly:
  no `Scope` **value** is erased to `'static` or storage-retyped in either
  crate. (The erased scope *reference* in sealed carriers, and the born door's
  exit re-anchor of one, are the substrate's normal reference discipline and
  remain.)
- `Scope`'s `Drop`-freedom is structural: every field is `Copy`, a `Cell` of a
  `Copy`, or a bump-backed table whose elements are held to those same bounds —
  no audited marker trait admits a `Drop`-bearing field, and the bump door that
  admits `Scope` states its bound in those terms.
- The `region_owner: Weak<FrameStorage>` field is deleted;
  `Scope::parent_frame_pin` and `Scope::region_owner` derive the owner from the
  region's own host back-link.
- `Scope` carries no writer: the run writer rides the run `CallFrame` beside
  the run's type registry, and `PRINT` reaches it through the execution
  context.
- No `ScopeKind` payload owns heap data or runs a destructor: kind names are
  bumped `&str`, the SIG slot collector's vacuous map teardown is suppressed the
  way `Bindings`' is, and no `Rc` rides a kind.
- `FamilyArena`, `Stored`, `FamilyList`, `StorageOf`,
  `StorageProfile::Families`, and `alloc_resident`'s typed path are deleted
  from workgraph; `typed-arena` leaves `workgraph/Cargo.toml`.
- Pinning semantics are unchanged: anything pinning the region pins the
  storage, which owns the bump; captured closures and module child scopes
  escape exactly as before.
- The Koan Miri slate and the workgraph slate
  (`cargo +nightly miri test -p workgraph --lib`) are both green, with slate
  coverage for the bumped, interior-mutable scope (the leak claim `Copy`
  bounds cannot state).

**Directions.**

- *Structural `Drop`-freedom — decided.* Composition over audit: `Scope` is
  built from `Copy` fields, `Cell`s, and glue-free bump-backed tables (the
  [`BumpAllocator`](../../workgraph/src/witnessed/bump.rs)
  no-drop-glue discipline), so the
  forgone destructor would have freed only bump bytes — no unsafe
  "trust me, it's `Drop`-free" marker tier is introduced.
- *Writer home — decided.* Context-carried, koan-only: the writer rides the
  run `CallFrame` exactly as the run's type registry does, threaded to `PRINT`
  through the execution context. Workgraph is not involved — the writer is a
  stopgap (see the note under Dependencies) and the library gains no slot for
  it.
- *Scope's bump door — decided.* Generalize the born-with shape onto the bump:
  one `RegionHandle::bump_born_with` (crossing-operand form only) whose bound
  is a monomorphization-checked `!needs_drop` assert and whose exit re-anchors
  the freshly bumped *reference* under the `&'a` region borrow through the
  substrate's single audited reattach. Its only callers are the per-call frame
  child and the transparent `USING` window; every same-region child and the
  run root are built directly at `'a` and stored through a new glue-free
  non-`Copy` allocator verb, `BumpAllocator::in_place`.

## Dependencies

The `PRINT` writer this item relocates is itself a stopgap until
[monadic side effects](../libraries/monadic-side-effects.md) replace direct
writer plumbing — not a dependency edge in either direction; this item only
moves the writer, it does not change how output is expressed.

**Requires:** none — the binding tables already shed their `Drop`
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)).

**Unblocks:** none tracked yet.

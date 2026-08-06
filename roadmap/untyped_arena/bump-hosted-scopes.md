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
What keeps `Scope` out of the bump is its `Drop`: the binding tables (their own
item), the per-scope `region_owner: Weak<FrameStorage>` field — a per-scope
copy of the fact the region already stores as its `host` back-link — the
root-only `Box<dyn Write>` writer, and the `String` /
`Rc<RecursiveGroupWindow>` payloads riding `ScopeKind`.

**Acceptance criteria.**

- `Scope` is allocated into its region's bump at the caller's `'a` directly:
  no `erase_to_static` call and no `Scope<'static>` ⇄ `Scope<'a>` storage
  retype exists in either crate.
- `Scope`'s `Drop`-freedom is structural: every field is `Copy`, a `Cell` of a
  `Copy`, or a bump-backed table whose elements are held to those same bounds —
  no audited marker trait admits a `Drop`-bearing field, and the bump door that
  admits `Scope` states its bound in those terms.
- The `region_owner: Weak<FrameStorage>` field is deleted;
  `Scope::parent_frame_pin` and `Scope::region_owner` derive the owner from the
  region's own host back-link.
- The root writer lives beside the run-root storage, not on `Scope`;
  `write_out` reaches it through the region owner.
- No `ScopeKind` payload owns heap data: kind names are bumped `&str`, the SIG
  slot collector is a bump-backed table, and no `Rc` rides a kind.
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
  built from `Copy` fields, `Cell`s, and element-`Copy` bump tables (the
  [`BumpMap`](../../workgraph/src/witnessed/bump.rs) discipline), so the
  forgone destructor would have freed only bump bytes — no unsafe
  "trust me, it's `Drop`-free" marker tier is introduced.
- *Writer home — decided.* The run-root storage side owns the root writer.
  `FrameStorage` is a library alias (`RegionHost<KoanStorageProfile>`), so the
  attachment is koan-side — see the open bullet.
- *Writer attachment — open.* A koan wrapper over the run-root
  `Rc<FrameStorage>` versus a workload-supplied slot on the storage profile.
- *Scope's bump door — open.* Generalize the existing born-with shape
  (`alloc_resident_born_with`'s `for<'b>` brand plus crossing operand) onto the
  bump, versus a scope-specific door. Recommended: generalize — the crossing
  machinery already exists and the erase step is the only part that drops out.

## Dependencies

**Requires:**

- [Bump-backed binding tables](bump-backed-bindings.md) — the tables must shed
  their `Drop` before `Scope` can skip its destructor.
- [Module bodies announce type groups](../type_language/module-announced-type-groups.md)
  — retiring `RECURSIVE TYPES` removes the `Rc`-carrying `RecursiveBlock`
  scope kind.

**Unblocks:** none tracked yet.

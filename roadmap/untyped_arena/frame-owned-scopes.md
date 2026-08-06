# Frame-owned scopes retire the typed cells

Retires the `typed-arena` dependency and workgraph's typed-storage machinery by
rehoming the last typed family. Storage vocabulary is defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary);
the current typed tier is described in
[workgraph/design/witnessed-memory.md](../../workgraph/design/witnessed-memory.md).

**Problem.** With `KFunction` and `Module` `Drop`-free, `Scope` is the last
typed family — and it is not a value-channel citizen. It is infrastructure
whose liveness unit is already `Rc<FrameStorage>`: the storage owns the region,
every pin is a pin on the storage, and region death is frame death. Hosting it
*inside* the region is what forces workgraph to carry the whole typed-storage
machinery — `FamilyArena`, `Stored`, `FamilyList` / `StorageOf`,
`StorageProfile::Families`, `alloc_resident`'s typed erase path — for one
embedder family. A scope does not need to be `Drop`-free; it needs to live
exactly as long as the region, which frame ownership gives it directly.

**Acceptance criteria.**

- `FrameStorage` owns its scopes beside the region in an append-stable,
  shared-reference home; scope drops run at frame death, the same schedule as
  region death.
- Pinning semantics are unchanged: anything pinning the region pins the
  storage, which owns the scopes; captured closures and module child scopes
  escape exactly as before.
- `FamilyArena`, `Stored`, `FamilyList`, `StorageOf`,
  `StorageProfile::Families`, and `alloc_resident`'s typed path are deleted
  from workgraph; `typed-arena` leaves `workgraph/Cargo.toml`.
- The `Scope<'static>` ⇄ `Scope<'a>` retype survives as one named koan-side
  site; net `unsafe` across the two crates does not grow.
- The Koan Miri slate and the workgraph slate
  (`cargo +nightly miri test -p workgraph --lib`) are both green.

**Directions.**

- *Scope home — decided.* `elsa::FrozenVec<Box<Scope>>` on `FrameStorage` —
  append-through-`&self` with stable addresses, the same
  append-stable-address argument the reach side table already rests on.

## Dependencies

**Requires:**

- [Drop-free `Module`](drop-free-module.md)

**Unblocks:**

- [Bump-backed binding tables](bump-backed-bindings.md)

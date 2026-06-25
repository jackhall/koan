# `alloc_ktype` returns `Witnessed`

Migrate the type allocation family onto `yoke`, so every `KType` born in a per-call region comes
back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_ktype`](../../src/machine/core/arena.rs) (~38 call sites — the
highest-volume family) returns a bare `&'a KType`; like the object path, its co-location invariant
rides as a prose SAFETY note at the downstream `Witnessed::new` rather than as a `yoke`
guarantee, even though the constructor and production witness plumbing now exist.

**Acceptance criteria.**

- `alloc_ktype` returns a `KType` bundled with its owning frame's witness, sourced through `yoke`,
  so a region-resident type is born co-located by construction.
- No production `Witnessed::new` site on the type path keeps a caller-asserted co-location SAFETY
  note where `yoke` now applies.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the plumbing — decided.* Built over
  [alloc-witness-plumbing](alloc-witness-plumbing.md); this item is the type-family conversion.
- *Separate from the object family — decided.* At ~38 sites the `ktype` conversion is its own PR
  rather than sharing one with [alloc-object](alloc-object-witnessed.md).

## Dependencies

**Requires:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) —
  supplies the threaded `Rc` and production witness impls this family conversion needs.

**Unblocks:** none.

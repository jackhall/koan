# `alloc_object` returns `Witnessed`

Migrate the object allocation family onto `yoke`, so every `KObject` born in a per-call region
comes back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_object`](../../src/machine/core/arena.rs) (~25 call sites) returns a
bare `&'a KObject`; the co-location invariant — that the witness pins *this* value's references —
rides as a prose SAFETY note at the downstream `Witnessed::new` bundle, not as a constructor
guarantee, even though [`Witnessed::yoke`](../../src/witnessed.rs) and the production witness
plumbing now exist.

**Acceptance criteria.**

- `alloc_object` returns a `KObject` bundled with its owning frame's witness, sourced through
  `yoke`, so a region-resident object is born co-located by construction.
- No production `Witnessed::new` site on the object path keeps a caller-asserted co-location
  SAFETY note where `yoke` now applies.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the plumbing — decided.* The owning-`Rc` threading and `WitnessRegion` /
  `MergeWitness` impls land in [alloc-witness-plumbing](alloc-witness-plumbing.md); this item is
  the object-family conversion over that foundation.

## Dependencies

**Requires:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) —
  supplies the threaded `Rc` and production witness impls this family conversion needs.

**Unblocks:** none.

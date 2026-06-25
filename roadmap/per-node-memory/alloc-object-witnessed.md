# `alloc_object` returns `Witnessed`

Migrate the object allocation family onto `yoke`, so every `KObject` born in a per-call region
comes back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_object`](../../src/machine/core/arena.rs) (~25 call sites) returns a
bare `&'a KObject` that is not witnessed at all: the co-location invariant — that the witness pins
*this* value's references — stays implicit in the region machinery, and a transitional
`Witnessed::new` bundle would assert it in prose rather than guarantee it by construction, even
though the [`Witnessed::yoke`](../../src/witnessed.rs) / `merge` constructors and the production
witness plumbing now exist.

**Acceptance criteria.**

- `alloc_object` returns a `KObject` bundled with its owning frame's witness, the object built
  inside the witness closure — region-pure parts via `yoke`, a referenced region-resident value (a
  list/dict element, a captured scope) folded in via `merge` against its carrier — so a
  region-resident object is born co-located by construction.
- The object family carries no `Witnessed::new`: a site referencing another witnessed value merges
  it rather than re-asserting co-location in prose.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the plumbing — decided.* The owning-`Rc` threading and `WitnessRegion` /
  `MergeWitness` impls land in [alloc-witness-plumbing](alloc-witness-plumbing.md); this item is
  the object-family conversion over that foundation.
- *Same construction-inversion as the pilot — decided.* The object is built inside the witness
  closure (`yoke` for region-pure parts, `merge` for a referenced witnessed value), not bundled
  after the fact; a `for<'b>` closure cannot accept an already-built `KObject<'a>`. See
  [alloc-witness-plumbing](alloc-witness-plumbing.md).

## Dependencies

**Requires:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) —
  supplies the threaded `Rc` and production witness impls this family conversion needs.

**Unblocks:** none.

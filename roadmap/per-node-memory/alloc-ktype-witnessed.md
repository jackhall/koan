# `alloc_ktype` returns `Witnessed`

Migrate the type allocation family onto `yoke`, so every `KType` born in a per-call region comes
back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_ktype`](../../src/machine/core/arena.rs) (~38 call sites — the
highest-volume family) returns a bare `&'a KType` that is not witnessed at all; like the object
path, a transitional `Witnessed::new` would assert co-location in prose rather than guarantee it by
construction, even though the `yoke` / `merge` constructors and the production witness plumbing now
exist.

**Acceptance criteria.**

- `alloc_ktype` returns a `KType` bundled with its owning frame's witness, built inside the witness
  closure — most `KType`s are owned / `Rc`-shared and `yoke` directly, while a region-referencing
  variant (a `KType::Module` naming its child scope) folds in via `merge` against that scope's
  carrier — so a region-resident type is born co-located by construction.
- The type family carries no `Witnessed::new`: a variant referencing another witnessed value merges
  it rather than re-asserting co-location in prose.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the plumbing — decided.* Built over
  [alloc-witness-plumbing](alloc-witness-plumbing.md); this item is the type-family conversion.
- *Separate from the object family — decided.* At ~38 sites the `ktype` conversion is its own PR
  rather than sharing one with [alloc-object](alloc-object-witnessed.md).
- *Same construction-inversion as the pilot — decided.* The type is built inside the witness
  closure; a `for<'b>` closure cannot accept an already-built `KType<'a>`. Most variants `yoke`
  (owned / `Rc` data); a `KType::Module` `merge`s its child-scope carrier. See
  [alloc-witness-plumbing](alloc-witness-plumbing.md).

## Dependencies

**Requires:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) —
  supplies the threaded `Rc` and production witness impls this family conversion needs.

**Unblocks:** none.

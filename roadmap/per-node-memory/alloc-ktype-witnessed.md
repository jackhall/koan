# `alloc_ktype` returns `Witnessed`

Migrate the type allocation family onto `yoke`, so every `KType` born in a per-call region comes
back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_ktype`](../../src/machine/core/arena.rs) (~38 call sites — the
highest-volume family) returns a bare `&'a KType` that is not witnessed at all; like the object
path, a transitional `Witnessed::new` would assert co-location in prose rather than guarantee it by
construction, even though the `yoke` / `merge` constructors and the production witness plumbing now
exist. The one region-referencing variant — a `KType::Module` naming its child scope — has its
liveness named only at a node boundary, by [transfer-into-lift](transfer-into-lift.md)'s structural
walk (the `KType::Module { frame }` per-value anchor), rather than folded onto its carrier at
construction.

**Acceptance criteria.**

- `alloc_ktype` returns a `KType` bundled with its owning frame's witness, built inside the witness
  closure — most `KType`s are owned / `Rc`-shared and `yoke` directly, while a region-referencing
  variant (a `KType::Module` naming its child scope) folds in via `merge` against that scope's
  carrier — so a region-resident type is born co-located by construction.
- The type family carries no `Witnessed::new`: a variant referencing another witnessed value merges
  it rather than re-asserting co-location in prose.
- A lifted `KType::Module`'s reached region is read off its carrier's witness set, retiring the type
  arm of [transfer-into-lift](transfer-into-lift.md)'s structural walk (its `KType::Module { frame }`
  per-value anchor).
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the shipped substrate — decided.* Built over the production witness impls and the unified
  `FrameSet` set-witness (see
  [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure));
  this item is the type-family conversion.
- *Separate from the object family — decided.* At ~38 sites the `ktype` conversion is its own PR
  rather than sharing one with [alloc-object](alloc-object-witnessed.md).
- *Construction inversion, not post-hoc bundling — decided.* The type is built inside the witness
  closure; a `for<'b>` closure cannot accept an already-built `KType<'a>`. Most variants `yoke`
  (owned / `Rc` data); a `KType::Module` `merge`s its child-scope carrier.
- *The scope witness rides the type, not `alloc_scope` — decided.* A `KType::Module`'s child scope is
  alloc'd via `alloc_scope`, but the witness that keeps its region alive rides the `KType::Module`
  carrier (the value), not the scope handle: so this inversion is the `KType` value's, and
  `alloc_scope` itself stays bare `&'a`. The merge operand is the scope carrier minted from the
  module's frame `Rc`, the same shape as the object channel's captured-scope merge.
- *The within-node value channel must carry the witness set — open.* As with
  [alloc-object](alloc-object-witnessed.md), `alloc_ktype`'s `merge` has a carrier operand only once
  the `KType` channel threads the `FrameSet` rather than the bare `&'a` plus per-value anchor.
  Recommended: settle the channel before scheduling.

## Dependencies

**Requires:**

- [`transfer_into` and closing the lift relocation unsafe](transfer-into-lift.md) — lands the
  per-carrier witness set and the structural walk this inversion folds into and retires.

**Unblocks:** none.

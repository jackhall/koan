# Delete the audited reattach tier

**Problem.** The three `unsafe impl AuditedStored` blocks in
[residence.rs](../../src/machine/core/arena/residence.rs) are koan's only
non-test `unsafe`: per-family `ptr::eq` residence audits consumed by
`alloc_resident_checked` behind `RegionBrand::alloc_function` / `alloc_scope` /
`alloc_module` and `build_frame_child_witnessed`
([arena.rs](../../src/machine/core/arena.rs),
[frame.rs](../../src/machine/core/arena/frame.rs)). At every call site the
destination brand and the stored value's region borrow derive from the same
object: a `KFunction` is allocated through its captured scope's own brand, a
same-region child `Scope` inherits `outer.brand` and stores through that same
brand, a `Module` through its child scope's chain, and the frame child is built
over `RegionBrand(handle_b)` two lines before the store. Each audit therefore
re-verifies at runtime an identity the site establishes by construction — it
can never fail, its decline arm is an unreachable `.expect` panic path, and the
stores ride the checked tier where a compile tier is reachable. The
compile-enforced shape already exists for one family:
`FoldingBrand::alloc_module_folded` discharges the transparent-ascribe module
store through `FoldedPlacement::alloc_resident_folded` with no audit.

**Acceptance criteria.**

- [residence.rs](../../src/machine/core/arena/residence.rs) does not exist:
  no koan-side `AuditedStored` impl remains, and non-test `src/` code contains
  no `unsafe`.
- Every `KFunction` / `Scope` / `Module` store discharges residence at compile
  time — construction and store fused at a brand-confined door (a fold-free
  born door or a fold placement) — with no runtime residence audit and no
  residence `.expect` panic path.
- A `compile_fail` fixture (or doctest) shows the born door rejects a value
  whose region pointer derives from an ambient (non-brand) lifetime.
- The Miri fixtures that pin the audited tier's raw-pointer erasure behavior
  ([arena/tests.rs](../../src/machine/core/arena/tests.rs),
  [module/tests.rs](../../src/machine/model/values/module/tests.rs)) are
  retired or re-target the born doors.

**Directions.**

- Door shape — open. (a) A workgraph fold-free born door —
  `RegionHandle::alloc_resident_born<K>(build: impl for<'b> FnOnce(FoldedPlacement<'b, W>) -> K::At<'b>)`-shaped
  — sound by the same no-outlives argument as the folded sinks: inside the
  `for<'b>` closure, the only `&'b Region` inhabitants derive from the
  capability handed in, so the built value's region pointer is the
  destination's by construction. (b) Koan-side fusion: each constructor
  allocates through a crate-internal placement and returns the stored
  `&'a Scope<'a>` / `&'a KFunction<'a>` / `&'a Module<'a>` directly.
  Recommended: (a) — one door covers every construct-at-destination-brand
  site.
- Operand crossing — open. The parent / captured scope is an operand the born
  closure needs re-anchored at the brand (`&'b Scope<'b>`), where the call
  sites hold a bare `&'a Scope<'a>`, not a witnessed carrier. Candidates: the
  fold engine's operand-view machinery (`StepContext::alloc_with_handle`), or
  a composition of the existing `erase_to_static` + `with_branded_ref`
  re-anchor primitives.
- Site conversion order — open. The frame-child store is self-contained (the
  scope borrows only what the door already brands together) and can land
  first to prove the door; the `alloc_function` / same-region-child /
  `alloc_module` sites follow as a sweep once the operand-crossing shape is
  settled.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.

# Eliminating the workgraph escape hatch

**Problem.** `RegionHandle::alloc_resident_audited`
([workgraph/src/witnessed/region.rs](../../workgraph/src/witnessed/region.rs))
is a safe `pub` method that stores a region-borrowing value gated only by a
caller-supplied audit closure — `|_, _| true` reproduces the unvetted
lifetime-lengthening move-in with no `unsafe` anywhere. The koan veneer layer
carries three exactly-such always-true sites today
(`FoldingBrand::alloc_ktype_folded`, `FoldingBrand::alloc_object_folded`, and
the frame-child door inside `build_frame_child_witnessed`, all in
[arena.rs](../../src/machine/core/arena.rs)), each sound only by a
comment-level confinement argument the compiler never checks. Separately,
`RegionBrand::handle()` hands any koan crate code the raw `RegionHandle`,
bypassing every veneer; eleven reaches exist across `src/machine`.

**Acceptance criteria.**

- `workgraph`'s public API has no safe function that stores a
  region-borrowing value gated only by a caller-supplied closure;
  `RegionHandle::alloc_resident_audited` in its current shape is absent from
  the `pub` surface.
- A permissive move-in (today's `|_, _| true`) is not expressible from safe
  code against the published surface; a `compile_fail` doctest pins the
  rejection.
- The three always-true audit sites in
  [arena.rs](../../src/machine/core/arena.rs) compile through the
  replacement surface with their confinement argument carried by a type
  (capability or witness), not by a closure plus comment.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Replacement mechanism — open.* (a) A per-family audit on the `Stored`
  trait, so the library itself runs the residence check at every audited
  move-in and the embedder writes it once per family rather than per call
  site; (b) brand-confined capability doors in the veneer layer (the
  `FoldingBrand` pattern) with the closure-gated method deleted; (c)
  `unsafe fn` at the boundary — relabels the hole rather than eliminating
  it, fallback only; (d) unpublish the method (koan-only visibility),
  deferring the embedder story. Recommended: (a) for general move-ins plus
  (b) for the fold and frame-child doors.
- *Raw-handle confinement — open.* Whether `RegionBrand::handle()` reach is
  in scope here: (a) confine raw-handle access to `src/machine/core` as part
  of this item; (b) leave it as a koan-side follow-up, since the handle is
  `pub(crate)` and not part of the published workgraph surface.

## Dependencies

The typed frame parenting the frame-child door's replacement rests on has
shipped — `CallFrame::new` derives the parent pin and `new_tail` reserves the
no-chain case
([design/per-call-region/frames.md § Outer-frame chain](../../design/per-call-region/frames.md)).
The witness-derived fused bind doors that front the evidence-tier audits have
also shipped
([design/witness-hosting.md § Scope and bindings](../../design/witness-hosting.md)).

**Requires:**


**Unblocks:**

- [Publishing the workgraph crate](../scheduler_library/workgraph-extraction.md)
  — the API freeze publishes the hatch-free surface.

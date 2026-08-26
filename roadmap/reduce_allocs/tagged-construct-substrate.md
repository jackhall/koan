# Tagged construction substrate

**Problem.** The dhat profile of a union-carrying tail loop (2026-08-25, one tagged
construction per iteration) attributes ≈15 heap allocations per construction to the
value-substrate seam: `Sectioned::build` under `section_cells`
([src/machine/model/values/kobject.rs](../../src/machine/model/values/kobject.rs),
≈12/construction via `alloc_payload`) builds the payload substrate through heap
containers rather than bump-side; `construct_tagged` and `launch`
([src/machine/execute/decide/constructors.rs](../../src/machine/execute/decide/constructors.rs))
allocate once each; and the construct path clones a `TypeNode` out of
`TypeRegistry::node` (1/construction) where a borrow or a `Copy` view would do. The
`tagged_construct` term in [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt)
prices the whole construction; this seam is its largest share not owned by another item.

**Directions.**

- *Substrate build transients — open.* Whether `Sectioned::build`'s working containers
  move to the step scratch arena or the section layout is precomputed on the type so the
  build is a single sized bump. Attribution below `Sectioned::build` first.
- *Registry node access — open.* `TypeRegistry::node` returning a clone on this path:
  a borrowing accessor, or a `Copy` projection of the variants the construct path reads.

**Acceptance criteria.**

- A tagged construction allocates no heap container in the substrate seam: a dhat
  re-profile of `audit/shapes/tagged_construct_calls40.koan` attributes no
  per-construction term to `Sectioned::build`, `section_cells`, or a `TypeNode` clone
  under `TypeRegistry::node`.
- The `tagged_construct` term in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drops by the removed
  share, and its bound in `tests/allocation_baseline.rs` is re-measured.

## Dependencies

**Requires:** none.

**Unblocks:** none.

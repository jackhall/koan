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

**Directions.** Settled 2026-08-26; the working plan is
`scratch/tagged-construct-substrate-plan.md` (untracked).

- *Substrate build transients.* `Sectioned::build` takes its inputs as an
  `ExactSizeIterator` and stages `cells`/`runs` as exactly-reserved `BumpVec`s in the
  destination region, leaked into the stored slices — the idiom `alloc_record` already
  uses; `section_cells` and `launch` stream instead of collecting. Precomputing the
  section layout on the type is not viable: the run partition depends on per-value reach
  verdicts the type cannot know.
- *Registry node access.* Per-query verbs on `TypeRegistry` (`is_union`,
  `union_variant_target`, `union_member_named`) returning `Copy` data, each confining the
  node-table borrow inside the registry method; cold error paths keep the `node()` clone.

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

# Raw-handle confinement

Confine raw `RegionHandle` access to `memory`, so the veneer is the only
door koan code stores through.

**Problem.** `pub(crate)` `RegionBrand::handle()`
(`region.rs`) hands any koan crate code the raw
`RegionHandle`, bypassing every veneer door. `memory` owns the substrate and
every Koan-bound name over it, so the brand door belongs to `memory` too —
but twenty-nine production reaches sit outside it. Most are in
`machine::core`: sixteen in [reach.rs](../../src/machine/core/scope/reach.rs),
seven in [scope.rs](../../src/machine/core/scope.rs), and one each in
`core/seals.rs`, `core/scope/registry.rs`, `core/scope/copy.rs`,
`core/kfunction.rs` and `core/kerror.rs`. The rest are spread across
`execute/decide/literal.rs`, `execute/decide/constructors.rs`,
`execute/outcome.rs`, `model/values/kobject.rs` and `model/values/coerce.rs`.
One more sits in [`machine::execute::step`](../../src/machine/execute/step.rs),
the designated second substrate seam.

Most routes the raw handle exposes are vetted by their own signature — the
`!needs_drop`-asserted `in_place` / `frozen_table` verbs, the rank-2
`bump_born_with` door, and the `mint_retained` composition — but the
`Copy`-bounded bump primitives (`value` / `slice`) vet drop glue only: nothing
in their signature mints a reach for a region-borrowing `Copy` value, so a
foreign-branded store there is held off by lifetime discipline, not by a
reach. Confining the brand door alone does not type the reach shut: workgraph
publishes `RegionHandle::from_owner` and `FoldedPlacement::handle()` as `pub`,
so any koan module holding an `Rc<FrameStorage>` or a placement can mint a raw
handle without it.

**Acceptance criteria.**

- No production call site outside `memory` obtains a raw `RegionHandle` — by
  any route (`RegionBrand::handle()`, `RegionHandle::from_owner`,
  `FoldedPlacement::handle()`) — except inside
  [`machine::execute::step`](../../src/machine/execute/step.rs), which spells
  the substrate by design.
- `RegionBrand::handle()` is not visible outside `memory`.
- The outside-`memory` work the raw handle served routes through veneer doors
  whose signatures carry the vetting: the shared destination-operand door, the
  operand-seed constructors, a `FoldingBrand`-fronted section build, and a
  `RegionBrand`-level host accessor.
- Production koan code cannot reach the workgraph raw mints
  (`RegionHandle::from_owner`, `FoldedPlacement::handle()`); tests reach a
  handle only through a `test-hooks`-gated accessor.
- The confinement adds no koan-side `unsafe impl`: `src/`'s production code
  still carries no `unsafe` at all.

**Directions.**

- *Boundary is `memory`, not `machine::core` — decided.* The veneer and the
  brand it fronts are `memory` items, so the module that owns the substrate
  owns the raw door. This puts `machine::core`'s scope and reach machinery
  outside the boundary, which is what makes the item's bulk the `reach.rs` and
  `scope.rs` sites rather than the handful in `execute` and `model`.
- *Outside-`memory` replacement — decided.* Route the outside sites through
  `Scope::dest_operand` ([reach.rs](../../src/machine/core/scope/reach.rs)),
  already `pub(crate)`, and add veneer doors for the shapes it does not cover.
  The alternative (koan-side `RegionBrand`-headed witness families over a
  local `unsafe impl HasRegionHandle`) is rejected: it puts `unsafe` into
  koan's production code.
- *Typed doors live with their payload — decided per the `memory` module's own
  rule.* A new veneer door that binds a payload type is an inherent
  `impl RegionBrand<'a>` / `impl FoldingBrand<'a>` block written in that
  payload's file, as `alloc_object_folded` and `alloc_cell_folded` already
  are — not a method added to `region.rs`.
- *Workgraph-side mints — open.* (a) Narrow `RegionHandle::from_owner` and
  `FoldedPlacement::handle()` to `pub(crate)` with a `test-hooks` accessor;
  (b) keep them `pub` for embedders and gate koan's use by module discipline
  plus a lint. Recommended: (a) — (b) leaves the reach open by construction,
  and the published-surface cost falls on workgraph's own embedder story.

## Dependencies

**Requires:** none — visibility and veneer work over existing doors.

**Unblocks:** none tracked.

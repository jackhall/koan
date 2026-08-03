# Born-witnessed frame-child store

**Problem.** `build_frame_child_witnessed`
([arena.rs](../../src/machine/core/arena.rs)) constructs the per-call child
scope over `RegionBrand(handle_b)` and then stores it through
`alloc_resident_checked::<Scope>`, whose `ptr::eq` family audit re-verifies at
runtime an identity the door itself establishes two lines earlier —
`child.region()` is `handle_b`'s own region by construction. The audit can
never fail, so its decline arm is an unreachable
`.expect("frame child is built over this frame's own region")` panic path, and
the store rides the checked tier where a compile tier is reachable.

**Acceptance criteria.**

- The frame-child store runs no runtime residence audit: the child scope's
  region identity is discharged by the type, with construction and store fused
  (a fold-free born door or an equivalent brand-confined shape).
- The `.expect("frame child is built over this frame's own region")` panic
  path does not exist.
- A `compile_fail` fixture (or doctest) shows the born door rejects a value
  whose region pointer derives from an ambient (non-brand) lifetime.

**Directions.**

- Door shape — open. (a) A workgraph fold-free born door —
  `RegionHandle::alloc_resident_born<K>(build: impl for<'b> FnOnce(FoldedPlacement<'b, W>) -> K::At<'b>)`-shaped
  — sound by the same no-outlives argument as the folded sinks: inside the
  `for<'b>` closure, the only `&'b Region` inhabitants derive from the
  capability handed in, so the built value's region pointer is the
  destination's by construction. (b) Koan-side fusion:
  `Scope::child_for_frame_witnessed` allocates through a crate-internal
  placement and returns the stored `&'a Scope<'a>` directly.
  Recommended: (a) — reusable by any other construct-at-destination-brand
  site.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.

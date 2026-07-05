# Allocation capability: the library region handle

**Problem.** The sole non-step allocation capability, `RegionBrand`, is a
Koan-side newtype over `&KoanRegion` (`src/machine/core/arena.rs`): its
minting rule (`FrameStorage::brand`, arena.rs:526) and the
no-alloc-from-bare-region confinement are Koan conventions, not library
types — the generic engine's `alloc` / `alloc_resident` are public on
`Region`, so a bare `&Region` allocating is prevented only by Koan-side
discipline. `Scope` stores that brand plus a `Weak<FrameStorage>`
(`src/machine/core/scope.rs:44`) as its own allocation path. The design
boundary assigns capability minting and confinement to the library, with
`Scope` reduced to a naming layer allocating through library handles held
by `CallFrame`.

**Acceptance criteria.**

- The non-step allocation capability is a library type: its minting (from a
  region owner) and the bare-region-cannot-allocate confinement rule live
  in `workgraph` and are compile-enforced — a bare `&Region` reaches no
  allocation surface outside the library, guarded by `compile_fail`
  doctests.
- Koan keeps at most typed `alloc_*` veneers over its own families,
  carrying no capability rules of their own.
- `Scope` is a naming layer — lookup, binding, shadowing; the handle its
  storage allocates through is a library type held by (or derived from)
  `CallFrame`, not a Koan-minted capability.
- Reach-set contents and region-liveness semantics are unchanged — existing
  tests green.

**Directions.**

- *Capability shape — open.* (a) `RegionBrand` becomes a Koan newtype
  veneer over a generic library handle, keeping the typed `alloc_*`
  wrappers on top so `scope.brand().alloc_*` call sites stay unchanged;
  (b) `RegionBrand` is deleted and call sites use the library handle
  directly through an extension trait for the typed veneers. Recommended:
  (a) — (b) forces a trait import into every builtin file that allocates.
- *Handle threading — open.* Where `Scope` gets its handle: (a) stored at
  construction (as today, but wrapping a library type); (b) re-derived from
  the frame at each allocation site. Recommended: (a) — re-derivation needs
  a `Weak` upgrade per site and a temporary `Rc` cannot hand back an
  `'a`-lived handle.

## Dependencies

**Requires:** none — the storage-bundle prerequisite has shipped.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the last
  boundary move that reshapes the library's public surface.

# Regions wholesale: the at-will allocation surface

**Problem.** The boundary in
[design/scheduler-library.md](../../design/scheduler-library.md) assigns
regions wholesale to the library, with `Scope` reduced to a naming layer
allocating through library region handles held by `CallFrame`. Today the
at-will (non-step) allocation surface is Koan-side plumbing in
`src/machine/core/arena.rs`: the storage bundle the library `Region` engine
runs is Koan-composed raw `typed_arena::Arena` fields (`KoanStorage`,
arena.rs:40) with per-family `Stored` impls indexing into them; the sole
non-step allocation capability, `RegionBrand`, is a Koan-side newtype over
`&KoanRegion` whose minting rule (`FrameStorage::brand`, arena.rs:526) and
no-alloc-from-bare-region confinement are Koan conventions, not library
types; and `Scope` stores that brand plus a `Weak<FrameStorage>`
(`src/machine/core/scope.rs:44`) as its own allocation path. The library
does the erase-store work, but arena ownership, capability minting, and
confinement all sit in Koan.

**Acceptance criteria.**

- The non-step allocation capability is a library type: its minting (from a
  region owner) and the bare-region-cannot-allocate confinement rule live in
  `workgraph`; Koan keeps at most typed `alloc_*` veneers over its own
  families, carrying no capability rules of their own.
- No Koan-side arena ownership remains: `typed_arena` is not a direct
  dependency of the `koan` crate, and the per-family sub-arena storage is
  owned by library types, with Koan's profile supplying only the family set
  and storage policy.
- `Scope` is a naming layer — lookup, binding, shadowing; the handles its
  storage allocates through are library types held by (or derived from)
  `CallFrame`, not a Koan-minted capability.
- Reach-set contents and region-liveness semantics are unchanged — existing
  tests green.

**Directions.**

- *Storage-bundle shape — open.* How the profile supplies families without
  owning arenas: (a) a library-owned generic sub-arena bundle the profile
  parameterizes by its family list, with `Stored` keying into library-owned
  cells; (b) the profile keeps supplying a storage type, built from a
  library sub-arena newtype that owns the raw arena. Recommended: (a) — it
  is the reading under which "no koan-side arena ownership" holds without a
  relabeling.
- *Capability generalization — open.* Whether `RegionBrand` becomes an
  instantiation of a generic library handle (Koan keeping its typed
  wrappers on top), or is deleted with call sites using the library handle
  directly.
- *Handle threading — open.* Where `Scope` gets its handle: stored at
  construction (as today, but a library type) vs re-derived from the frame
  at each allocation site.
- *Phasing — open.* Whether the storage-bundle move and the capability
  generalization split into two PRs to keep each one reviewable.

## Dependencies

**Requires:** none — the boundary move runs on shipped substrate.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the last
  boundary move that reshapes the library's public surface.

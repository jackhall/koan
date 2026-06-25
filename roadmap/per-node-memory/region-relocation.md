# Relocate `Region<P>` into the `witnessed` module

Re-home the generic bump allocator beside the carrier it feeds, so the substrate's
storage engine and its liveness carrier live in one module.

**Problem.** The generic [`Region<P>`](../../src/machine/core/region.rs) allocator — already
generic over `StorageProfile` / `Stored` on `master` — lives in `machine/core`, a layer
below the [`witnessed`](../../src/witnessed.rs) carrier it feeds. The substrate's storage
engine and its carrier therefore sit in different modules, and a reader of `witnessed` cannot
see the allocator whose values it wraps.

**Acceptance criteria.**

- `Region<P>`, `StorageProfile`, and `Stored` live in the `witnessed` module; `machine`
  re-instantiates `KoanRegion = Region<KoanStorageProfile>` and the family `Stored` impls
  through a `pub use`, so the existing `alloc_*` call sites compile unchanged.
- The relocation adds no `unsafe` and changes no behavior — `cargo test`, `cargo clippy
  --all-targets`, and the full Miri slate stay green.

**Directions.**

- *Move, not rewrite — decided.* `Region<P>` is already generic; this item only changes the
  module it lives in, leaving a `pub use` shim in `machine` so no caller path changes.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.

# Witness value carriers at their construction site

Migrate every value construction — object channel and type channel — onto the witnessed alloc, so
each value is born wrapped in its carrier and folds reach via `yoke` / `merge` / `transfer_into`,
then delete the bare `alloc_* -> &'a` callers and `reattach_with`.

**Problem.** With the [witnessed alloc surface](../../design/per-node-memory.md) and its `finalize`
producer-fold in place — proven on one region-pure pilot — the remaining construction sites still split
two ways. The value-embedding seal sites re-anchor a captured, region-resident value through the loose
[`reattach_with`](../../src/witnessed.rs) wrapper
([`Scope::seal_value`](../../src/machine/core/scope.rs) / `seal_module`,
[`fn_def::finalize`](../../src/builtins/fn_def/finalize.rs),
[`KoanRegion::alloc_witnessed_embedding`](../../src/machine/core/arena.rs), and the test-only
[`extract_terminal`](../../src/builtins/test_support.rs)) — co-location asserted by the wrapper rather
than enforced by the brand. And ~40 builtin construction sites still allocate through the bare
`scope.region.alloc_object(…) -> &'a` / `alloc_ktype` leaf and use the reference directly, so those
values live outside the carrier until a downstream seal re-wraps them.

**Acceptance criteria.**

- Every object- and type-channel construction site builds its value through the witnessed alloc and
  folds reach via `yoke` / `merge` / `transfer_into`, rather than allocating a bare `&'a` or
  re-anchoring a captured value.
- The seal sites (`seal_value` / `seal_module` / `finalize` / `alloc_witnessed_embedding`) receive
  their value as a carrier witnessed at its construction site.
- No construction site holds a bare `&'a` from `alloc_*`; the only remaining `alloc_* -> &'a` calls
  are the build-at-a-brand leaf inside `yoke` closures. `reattach_with` is deleted, and no call site
  references it.
- `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Witness at the alloc/construction site — decided.* A value carrier is minted where the value is
  built (inside its `yoke` brand), so a seal folds an already-witnessed carrier and a construction
  composes carriers — this removes the re-anchor rather than relabelling it.
- *Per-site mechanical, once the surface exists — decided.* Each construction site migrates
  independently onto the witnessed alloc; the work is breadth across every value-embedding site, not
  depth, so it lands as its own item after the surface is in place.

## Dependencies

**Requires:** none — the witnessed alloc surface it builds on has shipped.

**Unblocks:**

- [One region handle, one access verb](single-open-verb.md) — once no caller holds a bare `&'a`, the
  build leaf can be confined behind the branded handle.

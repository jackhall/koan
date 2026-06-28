# Witness value carriers at their construction site

Witness value carriers where they are allocated, so the seal sites receive a carrier to fold rather
than re-anchoring an already-built value through the free wrapper, then delete `reattach_with`.

**Problem.** The value-embedding seal sites still re-anchor a captured, region-resident value through
the loose [`reattach_with`](../../src/witnessed.rs) wrapper (6 sites):
[`Scope::seal_value`](../../src/machine/core/scope.rs) / `seal_module`,
[`fn_def::finalize`](../../src/builtins/fn_def/finalize.rs),
[`KoanRegion::alloc_witnessed_embedding`](../../src/machine/core/arena.rs), and the test-only
[`extract_terminal`](../../src/builtins/test_support.rs). Each re-anchors a value built *outside* its
`yoke` brand, so co-location is asserted by the wrapper rather than enforced by the brand — the residue
the [witnessed substrate](../../design/per-node-memory.md) closes everywhere else. Until the values are
witnessed at their construction site — handed to the seal as carriers it folds via `yoke` / `merge` —
`reattach_with` stays alive as an alternate spelling of the one primitive.

**Acceptance criteria.**

- The seal sites (`seal_value` / `seal_module` / `finalize` / `alloc_witnessed_embedding`) receive
  their value as a carrier witnessed at its construction site and fold reach via `yoke` / `merge`,
  rather than re-anchoring a captured value.
- `reattach_with` is deleted, and no call site references it.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Witness at the alloc/construction site — decided.* A value carrier is minted where the value is
  built (inside its `yoke` brand), so the seal folds an already-witnessed carrier; this removes the
  re-anchor rather than relabelling it.
- *Own item, sequenced after the value-read migration — decided.* Making the seal sites structural
  touches every value-embedding construction, so it lands as its own item rather than riding the
  value-read migration.

## Dependencies

Builds on the shipped value-read migration (the result-slot value reads nest under `Sealed::open`).

**Requires:** none — the value-read migration shipped; this item witnesses the value carriers the
seal sites currently re-anchor.

**Unblocks:** none.

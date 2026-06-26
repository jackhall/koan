# Migrate the loose witness-borrow wrappers onto `Sealed`

Move the continuation / contract carriers and the value-path reference reattaches off the loose
`vend_carrier` / `reattach_with` / `reattach_ref_with` / `reattach_slice_with` functions and onto
the `Sealed` surface, deleting all four.

**Problem.** Four loose witness-borrow functions re-anchor a carrier against a passed witness
borrow: [`vend_carrier`](../../src/witnessed.rs), [`reattach_with`](../../src/witnessed.rs),
`reattach_ref_with`, and `reattach_slice_with`. The [keystone](runloop-cps-open.md) deletes their
run-loop-tail uses (the two `vend_carrier`, the `apply_outcome` `Outcome::Forward` `reattach_with`,
the `deps_at_step` slice). The remaining ~36 sites — the dispatch-decide `reattach_with` (~29), the
`scope_ptr` `reattach_ref_with` (~7), and any residual slice site — still route loose functions
rather than the keystone's `open`, leaving the four wrappers alive as alternate spellings of one
primitive.

**Acceptance criteria.**

- The remaining ~36 `vend_carrier` / `reattach_with` / `reattach_ref_with` / `reattach_slice_with`
  call sites read through the keystone's `open` (copy-out where the value does not escape) or
  [`attach`](externally-witnessed-attach.md) (only where a site proves it must ride up-stack); the
  four helper functions are deleted.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Prefer `open`, reach for `attach` — decided.* Each site favours `open` + copy-out to minimize the
  `attach` residue [remove-attach](remove-attach.md) must clear, reaching for `attach` only where a
  reference genuinely escapes the access.
- *One PR across the four wrappers — decided.* The remaining ~36 sites are a uniform mechanical
  change, so the four wrappers retire together rather than as separate near-identical items.

## Dependencies

**Requires:**

- [Consuming externally-witnessed `open` and the run-loop step restructure](runloop-cps-open.md) —
  supplies the `open` verb these sites move onto and deletes their run-loop-tail subset.

**Unblocks:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — one of the call sites that
  item surveys for an un-nestable non-scope reference.
- [Remove `attach`](remove-attach.md) — one of the migrations that must land before `attach` can be
  deleted.

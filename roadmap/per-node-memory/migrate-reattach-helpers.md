# Migrate the loose witness-borrow wrappers onto `Sealed`

Move the remaining value-path reference reattaches off the loose `reattach_with` / `reattach_ref_with`
functions and onto the `Sealed` surface, deleting both.

**Problem.** Two loose witness-borrow functions re-anchor a carrier against a passed witness
borrow: [`reattach_with`](../../src/witnessed.rs) and [`reattach_ref_with`](../../src/witnessed.rs).
The shipped keystone restructure deleted the run-loop tail's loose reattaches outright —
`vend_carrier` and `reattach_slice_with` are gone entirely. The remaining ~36 sites — the
dispatch-decide `reattach_with` (~29) and the `scope_ptr` `reattach_ref_with` (~7) — still route
loose functions rather than the shipped `open`, leaving the two wrappers alive as alternate spellings
of one primitive.

**Acceptance criteria.**

- The remaining ~36 `reattach_with` / `reattach_ref_with` call sites read through the shipped `open`
  (copy-out where the value does not escape) or [`attach`](externally-witnessed-attach.md) (only
  where a site proves it must ride up-stack); both helper functions are deleted.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Prefer `open`, reach for `attach` — decided.* Each site favours `open` + copy-out to minimize the
  `attach` residue [remove-attach](remove-attach.md) must clear, reaching for `attach` only where a
  reference genuinely escapes the access.
- *One PR across both wrappers — decided.* The remaining ~36 sites are a uniform mechanical
  change, so the two wrappers retire together rather than as separate near-identical items.

## Dependencies

**Requires:**


**Unblocks:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — one of the call sites that
  item surveys for an un-nestable non-scope reference.
- [Remove `attach`](remove-attach.md) — one of the migrations that must land before `attach` can be
  deleted.

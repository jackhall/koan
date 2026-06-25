# Migrate the loose witness-borrow wrappers onto `Sealed`

Move the continuation / contract carriers and the value-path reference reattaches off the loose
`vend_carrier` / `reattach_with` / `reattach_ref_with` / `reattach_slice_with` functions and onto
the `Sealed` surface, deleting all four.

**Problem.** Four loose witness-borrow functions re-anchor a carrier against a passed witness
borrow: [`vend_carrier`](../../src/witnessed.rs) (2 sites in `run_loop.rs` — the continuation at the
step boundary, the contract at `Done`) re-anchors the scheduler's `Erased` continuation / contract
carriers; [`reattach_with`](../../src/witnessed.rs) (~30 sites), `reattach_ref_with` (~7), and
`reattach_slice_with` (~4) re-anchor a live value, single reference, or slice. With `attach`
reimplementing them, these ~43 sites route loose functions rather than the `Sealed` method, leaving
four wrappers as alternate spellings of one primitive.

**Acceptance criteria.**

- The ~43 `vend_carrier` / `reattach_with` / `reattach_ref_with` / `reattach_slice_with` call sites
  read through `Sealed::open` (copy-out where the value does not escape) or `Sealed::attach` (only
  where a reference must ride up-stack); the four helper functions are deleted.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Prefer `open`, reach for `attach` — decided.* Each site favours `open` + copy-out to minimize the
  `attach` residue [remove-attach](remove-attach.md) must clear, reaching for `attach` only where a
  reference genuinely escapes the access.
- *One PR across the four wrappers — decided.* The ~43 sites are a uniform mechanical change, so the
  four wrappers retire together rather than as separate near-identical items.

## Dependencies

**Requires:**

- [Externally-witnessed sealed form and `attach`](externally-witnessed-attach.md) — supplies the
  `Sealed` access methods these sites move onto.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — one of the migrations that must land before `attach` can be
  deleted.

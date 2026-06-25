# Migrate `reattach_*_with` sites onto `Sealed`

Move the `outcome.rs` and value-path reference reattaches off the loose `reattach_with` /
`reattach_ref_with` / `reattach_slice_with` helpers and onto the `Sealed` surface, deleting them.

**Problem.** The witness-borrow reference reattaches —
[`reattach_with`](../../src/witnessed.rs) (~30 sites), `reattach_ref_with` (~7), and
`reattach_slice_with` (~4) — re-anchor a live value, single reference, or slice against a passed
witness borrow. With `attach` reimplementing them, these ~41 sites route loose functions rather
than the `Sealed` method, leaving three wrappers as alternate spellings of one primitive.

**Acceptance criteria.**

- The ~41 `reattach_with` / `reattach_ref_with` / `reattach_slice_with` call sites read through
  `Sealed::open` (copy-out where the value does not escape) or `Sealed::attach` (only where a
  reference must ride up-stack); the three helper functions are deleted.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Prefer `open`, reach for `attach` — decided.* As with the
  [`vend_carrier` migration](migrate-vend-carrier.md), each site favours `open` + copy-out to
  minimize the `attach` residue [remove-attach](remove-attach.md) must clear.
- *One PR across the three helpers — decided.* The ~41 sites are a uniform mechanical change, so
  the three helpers retire together rather than as three near-identical items.

## Dependencies

**Requires:**

- [Externally-witnessed sealed form and `attach`](externally-witnessed-attach.md) — supplies the
  `Sealed` access methods these sites move onto.

**Unblocks:**

- [Remove `attach`](remove-attach.md) — one of the four carrier/read migrations that must land
  before `attach` can be deleted.

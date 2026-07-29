# Sectioned substrates

Route koan's composite substrates through workgraph's sectioned storage
([design/value-substrates.md § Sectioned reach](../../design/value-substrates.md#sectioned-reach)),
retiring the seam-time shape walks.

**Problem.** A substrate's cells carry no stored reach — only the whole
value's single description exists — so every seam that parts a cell from
its container re-derives reach by walking the value: the
[`product_still_borrows`](../../src/machine/core/scope/reach.rs) escape
probe at transfer, release-exact subset derivation at projection, and the
per-host address-table probe on the unpriceable release path.

**Acceptance criteria.**

- Koan's composite substrates section their cells through workgraph's alloc
  door, handing in each cell as a region-resident borrow paired with its
  copy-or-pin verdict, and supplying nothing else beyond the born-borrowing
  seeds (the `FN` door naming a closure's captured scope, the module door
  naming its child scope).
- The contains-borrows and borrows-home memos are folds over the
  substrate's run descriptions, not a separate walk.
- Dict keys are restricted to owned data, and the construction door
  rejects a borrow-carrying key by its stored envelope — an O(1) check,
  not a walk.
- A projection or index read hands a cell out bundled with exactly its own
  run's stored reach — read, not derived, and never a subset walk over the
  container. A mint happens only where the cell relocates, at the re-seal.
- The seam-time shape walks are deleted: the `product_still_borrows`
  escape probe, release-exact subset derivation at projection, and the
  per-host address-table release probe — a transfer claims the empty
  source bundle exactly when no surviving run names the source region.
- The Miri audit slate is green after the retirements.

## Dependencies

The residence-audit walks named in the design's "What this replaces"
retire under
[Residence-audit retirement](residence-audit-retirement.md)'s fold-brand
doors, not here.

**Requires:**


**Unblocks:** none tracked.

# `transfer_into` and closing the lift relocation unsafe

Recast the consumer-pull lift as a borrow-checked copy into the destination region,
retiring the one irreducible value-path `unsafe`.

**Problem.** When a consumer pulls a dep across a dependency edge,
[`lift`](../../src/machine/execute/lift.rs) copies the value into the consumer's region
and re-anchors any surviving borrow through `unsafe { reattach_value::<CarriedFamily>(value)
}` (lift.rs:48) plus `lift_kobject` (lift.rs:66) — the one audited `unsafe` reattach the
shipped witnessed carrier could not remove, because no borrowed witness is held for a value
about to be copied out.

**Acceptance criteria.**

- [`Sealed<T, W>`](sealed-open.md) has `transfer_into`, copying the sealed value into a
  destination region witnessed by the **destination** and returning it at the destination's
  lifetime — borrow-checked end to end.
- The consumer-pull lift routes `transfer_into`; the `reattach_value::<CarriedFamily>` call
  at `lift.rs:48` is deleted, dropping the value-path `unsafe` count by one.
- The Miri slate (including the lift/drain tests) is green; `cargo test` and
  `cargo clippy --all-targets` clean.

**Directions.**

- *Destination is the witness — decided.* The copy is witnessed by the region it lands in,
  so the borrow checker proves the result outlives nothing the destination does not pin; no
  fabricated source lifetime survives the copy.
- *Independent of `attach` — decided.* This closes a relocation seam, not an access seam; it
  shares only the `Sealed` type with
  [externally-witnessed-attach](externally-witnessed-attach.md) and can land in either order
  after sealed-open.

## Dependencies

**Requires:**

- [Sealed node-storage carrier and `open`](sealed-open.md) — the `Sealed` type this adds
  `transfer_into` to.

**Unblocks:** none.

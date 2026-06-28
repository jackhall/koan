# Carrier-self-building object constructions return `Witnessed`

Convert the object-construction sites that build their own carrier — the newtype / tagged-union
constructors, `catch`, and FN def — onto `yoke` / `merge` / `transfer_into`, so each names the reach
of every region it embeds on its carrier rather than pairing an already-built `&'a KObject` with an
asserted witness.

**Problem.** The region-pure and aggregate object constructions are built inside the witness closure —
a region-pure leaf [`yoke`s](../../src/witnessed.rs), and a list / dict / record folds its dep
[`Sealed`](../../src/witnessed.rs) carriers via `transfer_into`. But the carrier-self-building
constructions — the `NEWTYPE` / tagged [`constructors`](../../src/machine/execute/dispatch/constructors.rs),
[`catch`](../../src/builtins/catch.rs)'s `Result`, and the FN-def
[`finalize`](../../src/builtins/fn_def/finalize.rs) — still pair an already-built `&'a KObject` with an
asserted `Witnessed::new`. The constructors and `catch` hold their embedded values as dep terminals
(value + reach), and FN def holds a `KObject::KFunction` co-located in its defining scope's frame; both
have a carrier in hand to fold, yet the build reads a bare value out and re-pairs it with a separately
recovered reach. So these sites keep the object path *depending on* the read-out
[`reached_frame`](../../src/machine/execute/lift.rs) reconstruction to recover reach — the mechanism
the rest of the object family no longer needs.

**Acceptance criteria.**

- The newtype / tagged-union [`constructors`](../../src/machine/execute/dispatch/constructors.rs) build
  the `KObject::Wrapped` / `Tagged` **inside the witness closure**, folding their value deps' carriers
  via `transfer_into` (a record newtype folds each field carrier, then `merge`s the record) so the
  constructed object names every region it reaches; the nominal type identity crosses the build brand as
  a non-object [`RegionTypeFamily`](../../src/machine/core/arena.rs) operand, and the type-check runs
  before the build (read out of the bare dep value) so the closure is infallible.
- [`catch`](../../src/builtins/catch.rs)'s `Result` is built witnessed: the `Ok` arm `transfer_into`s
  the watched value's carrier, the `Error` arm `yoke`s the region-pure error tag and `merge`s the
  identity operand, and the finish returns `Action::DoneWitnessed` — never an asserted `Witnessed::new`
  over a read-out value.
- The FN-def [`finalize`](../../src/builtins/fn_def/finalize.rs) returns a
  `Witnessed<CarriedFamily, FrameSet>`: the co-located `KObject::KFunction` is re-anchored onto a
  carrier witnessed by the defining scope's frame via `yoke` + `reattach_with` (the captured scope —
  region-resident under the frame — transitively keeps every foreign region its bindings reach alive
  through the scope's sealed reach-set), and the three FN-def sites route it as
  `Action::DoneWitnessed`.
- These sites add no object-value `Witnessed::new`. (The non-object operand `Witnessed::new` that
  cross the build brand — `RegionTypeFamily` here, plus `RegionRefFamily` / `ContractHomeFamily`
  elsewhere — and the transitional value-copy `finalize_terminal` `Witnessed::new` are **not**
  object-value embeds and stay: "zero object `Witnessed::new`" is zero at an object-VALUE embedding
  site, decided per the *Directions* below.) The remaining object-value `Witnessed::new` — the
  bare-arg sites and the literal Resolved arm — convert under
  [the carrier-delivery follow-up](alloc-object-delivered-carriers.md).
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Construction inversion via dep carriers — decided.* Each converted site builds inside the witness
  closure: the constructors and `catch` fold their embedded dep carriers via `transfer_into` / `merge`,
  and FN def `yoke`s its co-located `KObject::KFunction` and re-anchors it via `reattach_with`. The
  decide-side constructors park on their value deps through a `park_on_deps_witnessed`
  ([`FinishWitnessed`](../../src/machine/execute/outcome.rs) continuation) that reuses the apply-side
  witnessed dep-finish, so a construction decide builds the wrapped value naming every region it
  reaches.
- *The nominal type identity crosses as a shared non-object operand — decided.* The declared / `SetRef`
  type identity a construction wraps with is type-channel data the consumer frame's `outer` chain pins,
  not an object value, so it crosses the build brand as the shared
  [`RegionTypeFamily`](../../src/machine/core/arena.rs) `(&'r KoanRegion, &'r KType<'r>)` operand
  consumed inside the `transfer_into` / `merge`.
- *"Zero object `Witnessed::new`" is zero at an object-VALUE embed — decided.* The non-object operand
  `Witnessed::new` (`RegionTypeFamily` here; `RegionRefFamily` for the relocation `dest`;
  `ContractHomeFamily` for the declared-return re-stamp) feed a `merge` / relocation and carry no object
  value, and the transitional value-copy [`finalize_terminal`](../../src/machine/execute/finalize.rs)
  `Witnessed::new` serves the type channel, errors, and `KType::Module`. The criterion targets the
  object-VALUE embedding sites; these helper / transitional carriers are exempt.

## Dependencies

This converts the object-construction sites that already hold a carrier; the value-embedding sites that
take a bare arg, and the relocate-seam-fold retirement, follow in
[the carrier-delivery follow-up](alloc-object-delivered-carriers.md).

**Requires:** none — the [per-scope reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into)
foundation and the dep-carrier delivery to construction finishes have shipped.

**Unblocks:**

- [Carrier-delivered object embeds and the relocate-seam-fold retirement](alloc-object-delivered-carriers.md) —
  the bare-arg value-embedding sites and the seam-fold retirement build on these carrier-self-building
  conversions.

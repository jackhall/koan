# Structural satisfaction

Signature-slot admission becomes `self_sig <: slot_sig` — pure, structural, requiring no
prior ascription.

**Problem.** Signature-slot admission is gated on recorded ascriptions:
[`matches_type`](../../src/machine/model/types/ktype_predicates.rs) checks membership in
`compatible_sigs`, so an unascribed module never matches a signature slot, and a module's
admissibility mutates over its lifetime as ascriptions accrue. The subtyping relation and
the creation-time self-sig (shipped by
[Signature subtyping and self-sigs](signature-subtyping-and-self-sigs.md)) exist, but
dispatch does not consult them.

**Acceptance criteria.**

- A signature-typed slot admits a module iff `self_sig <: slot_sig`; no prior ascription is
  required.
- `compatible_sigs` no longer exists; no mutable satisfaction state remains on `Module`.
- Tests that assert an unascribed module is rejected by a slot it structurally satisfies are
  updated to assert admission under the structural rule.

**Directions.**

- *Structural satisfaction — decided.* Ascription is assertion + view construction, never an
  admission gate. Implicit-candidate sets stay lexical
  ([design/typing/implicits.md](../../design/typing/implicits.md)), so coherence is
  unaffected.
- *Phasing — decided.* Foundation phase (carries the risk): flip `matches_type`'s signature
  arm from the `compatible_sigs` lookup to the subtype check — the behavioral change and its
  test fallout land together. Mechanical phases, each leaving the verify-koan slate green:
  delete `compatible_sigs` and its write paths, doc sweep.

## Dependencies

**Requires:**

- [Signature subtyping and self-sigs](signature-subtyping-and-self-sigs.md) — supplies the
  relation and the self-sig the flip consults.

**Unblocks:**

- [KObject module carrier](kobject-module-carrier.md) — value-side admission reuses the
  structural rule.

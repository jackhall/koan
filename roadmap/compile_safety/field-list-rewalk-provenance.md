# Field-list re-walk type provenance

**Problem.** The field-list elaborator re-walk (`FieldListRewalk::run`,
[field_list.rs](../../src/machine/execute/dispatch/field_list.rs)) rebuilds
field types from owned dep values and scope reads at the ambient step
lifetime, producing `(String, KType)` pairs whose reach is named only by a
parallel carriers list. Two finalizes store its output through the fold
surface with the composed type captured ambiently: `elaborate_record_value`'s
`alloc_type_with(carriers, KType::Record(…))` (field_list.rs) and the
deferred function-carrier finalize (`build_carrier` → `finalize_carrier` in
[parameterized_types.rs](../../src/builtins/parameterized_types.rs)). The
elaborated types are not addressable from the fold closure's dep views, so
these sites cannot build inside the fold brand — they are what forces the
folded placement methods (`alloc_ktype_folded` / `alloc_object_folded`,
[arena.rs](../../src/machine/core/arena.rs)) to accept any-lifetime values,
the capture hole [fold-closure capture
provenance](fold-closure-provenance.md) names.

**Acceptance criteria.**

- `elaborate_record_value`'s deferred arm and the deferred function-carrier
  finalize build their composite `KType` at the store's own fold brand, from
  the fold's declared operands (dep views, crossed operands, or scope reads
  opened at the same brand).
- No field-list site captures an ambient-lifetime `KType` into a folded
  placement: the synchronous `Done` arms store their ambient-composed type
  through the audited non-fold tier (`alloc_type_pure`). Migrating
  synchronous composition onto the brand is
  [checked-tier confinement](checked-tier-confinement.md), not this item.
- Each elaborated field type is paired with the carrier(s) it was elaborated
  from by the construction shape itself, not by a positional side-channel
  list.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Re-walk placement — decided.* (a): the deferred re-walk runs inside the
  fold closure — sub-dispatch types arrive as dep views, the consumer's scope
  crosses as its own `Delivered` envelope (`ScopeRefFamily`) opened at the
  brand, and the FN/FUNCTOR return type rides as an extra dep view from its
  arg carrier (region-free literal returns rebuild at the brand). The
  synchronous arms keep their single ambient walk and the audited non-fold
  store; the fold-brand requirement is deliberately scoped to the folded
  placements this item exists to clean.

## Dependencies

**Requires:** none — operates on the current fold surface.

**Unblocks:**

- [Fold-closure capture provenance](fold-closure-provenance.md) — the
  record/function-carrier finalizes must build from declared operands before
  the fold surface can reject captures.
- [Checked-tier confinement](checked-tier-confinement.md) — the at-brand
  construction door is the surface synchronous composition migrates onto.

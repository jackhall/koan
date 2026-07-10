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

- `elaborate_record_value` and the deferred function-carrier finalize build
  their composite `KType` at the store's own fold brand, from the fold's
  declared operands (dep views, crossed operands, or scope reads opened at
  the same brand); neither captures an ambient-lifetime `KType` into a
  folded placement.
- Each elaborated field type is paired with the carrier(s) it was elaborated
  from by the construction shape itself, not by a positional side-channel
  list.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Re-walk placement — open.* (a) Run the re-walk inside the fold closure
  against brand views — its scope reads are available there via the frame's
  own envelope, so the question is delivering the owned dep values at the
  brand; (b) cross each elaborated field type as a `RegionTypeFamily`-style
  operand after a scope-region alloc (the operand pattern
  [catch.rs](../../src/builtins/catch.rs) and
  [constructors.rs](../../src/machine/execute/dispatch/constructors.rs)
  already use); (c) restructure the re-walk to emit an owned (`'static`)
  schema with resident references resolved at store time. Recommended: spike
  (a) — it keeps one construction site and lands directly in the
  compile-enforced shape.

## Dependencies

**Requires:** none — operates on the current fold surface.

**Unblocks:**

- [Fold-closure capture provenance](fold-closure-provenance.md) — the
  record/function-carrier finalizes must build from declared operands before
  the fold surface can reject captures.

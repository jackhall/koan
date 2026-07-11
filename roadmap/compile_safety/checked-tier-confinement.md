# Checked-tier confinement to identity-preserving stores

**Problem.** The runtime-checked store tiers (`alloc_ktype_checked` /
`alloc_type_checked` and the `alloc_type_pure` / `alloc_ktype_pure` composite
entries, [arena.rs](../../src/machine/core/arena.rs); see
[memory-model.md § Move-in residence audits](../../design/memory-model.md#move-in-residence-audits))
audit a value's region borrows at runtime. Two caller classes ride them
today. The first is identity-preserving stores of values that cannot rebuild
at `'static` (a module-family pointer, an `Rc`-shared set whose `ptr_eq`
identity a rebuild would break) — the tier's reason to exist. The second is
synchronous *composition*: sites that assemble a new composite `KType` from
parts read ambiently and store it through the audit rather than building at a
brand from declared operands — `build_carrier`'s synchronous arm
([parameterized_types.rs](../../src/builtins/parameterized_types.rs)), the
`Signature` composition in
[type_ops/with.rs](../../src/builtins/type_ops/with.rs), and (once
[field-list re-walk provenance](field-list-rewalk-provenance.md) ships)
`elaborate_record_value`'s synchronous arm
([field_list.rs](../../src/machine/execute/dispatch/field_list.rs)). For the
composition class the runtime audit is a backstop where compile-time
enforcement is achievable: the same at-brand doors the deferred paths use.

**Acceptance criteria.**

- Every synchronous composite-`KType` construction — a site that assembles a
  new `KType` from parts it read ambiently — builds at a brand from declared
  operands (dep views, crossed operands, or scope reads opened at the same
  brand), not through a runtime residence audit.
- Every remaining caller of `alloc_ktype_checked` / `alloc_type_checked` /
  `alloc_ktype_pure` / `alloc_type_pure` stores an identity-preserving value
  that cannot rebuild at `'static`; a caller audit (grep of the four entry
  points) finds no composition site.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Migration surface — open.* (a) Reuse the field-list at-brand door (dep
  views + the scope crossed as its own `Delivered` envelope) for
  scope-reading composers; (b) per-site `RegionTypeFamily`-style operand
  crossings for sites whose parts already have carriers; (c) retire the
  `*_pure` composite entries in favor of explicitly-chosen tiers, so a new
  composition site cannot silently land on the runtime audit. Recommended:
  (a) for field-list-shaped sites, with (c) as the guard once the composition
  class is empty.

## Dependencies

**Requires:**

- [Field-list re-walk type provenance](field-list-rewalk-provenance.md) —
  the at-brand construction door (scope envelope + dep views) synchronous
  composition reuses.

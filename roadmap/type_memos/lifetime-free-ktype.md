# KType without a lifetime parameter

Delete `KType<'a>`'s lifetime parameter and the type-side residence machinery that
polices it. Part of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md).

**Problem.** `KType<'a>`'s lifetime parameter exists solely because `Signature`
holds region pointers behind `SigSource`
([`ktype.rs`](../../src/machine/model/types/ktype.rs)); once
[Signature types own their schema](signature-schema-ownership.md) ships, the
parameter constrains nothing yet still infects 69 files (405 `KType<` occurrences).
A whole type-side enforcement apparatus exists to police the pointers it threads:
`KType::to_static` and the `resident_in` walk family, an
`unsafe impl AuditedStored for KType`
([`residence.rs`](../../src/machine/core/arena/residence.rs)), the checked/reaching
`Scope` allocation tiers over plain `alloc_ktype`, and `StoredReach` evidence on
the type binding tables ([`bindings.rs`](../../src/machine/core/bindings.rs)).

**Acceptance criteria.**

- `KType` carries no lifetime parameter, and neither do the types parameterized
  only through it (`RecursiveSet`, `NominalMember`/`NominalSchema`, `SigSchema`,
  the signature/argument carriers).
- The type-side residence machinery is deleted: `KType::to_static`, the
  `resident_in`/`resident_in_reach`/`resident_in_visiting` walks, the
  `unsafe impl AuditedStored for KType`, the checked/reaching `Scope` KType
  allocation tiers, and the `StoredReach` component of the type binding tables;
  value-side residence auditing is untouched.
- KType allocation collapses to a single unchecked door into region storage; no
  `unsafe` impl remains for `KType`.
- The full test slate and the Miri audit slate are green.

**Directions.**

- *Storage stays by-reference — decided.* Binding tables and value carriers keep
  their region-allocated `&KType` references in this item; converting storage to
  by-value handles is
  [Interned type content behind Copy handles](interned-type-content.md)'s job, once
  the handle is `Copy`.

## Dependencies

**Requires:**

- [Signature types own their schema](signature-schema-ownership.md) — removes the
  region pointers that make the lifetime parameter load-bearing.

**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — a `Copy`
  digest handle requires a lifetime-free `KType`.

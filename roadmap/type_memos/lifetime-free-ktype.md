# KType without a lifetime parameter

Delete `KType<'a>`'s lifetime parameter and the type-side residence machinery that
polices it. Part of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md).

**Problem.** No `KType` variant borrows region data — every variant owns its content,
a signature's schema included ([`ktype.rs`](../../src/machine/model/types/ktype.rs)) —
so `KType<'a>`'s lifetime parameter constrains nothing yet still infects 69 files
(405 `KType<` occurrences). The type-side enforcement apparatus built to police
region pointers outlives the pointers themselves and is now vacuous: the
`resident_in`/`resident_in_reach` walk reaches no `owns_*` leaf and returns `true`
for every `KType`, so the `unsafe impl AuditedStored for KType`
([`residence.rs`](../../src/machine/core/arena/residence.rs)) admits every store and
its SAFETY argument rests on a tautology rather than a check. `KType::to_static`,
the checked/reaching `Scope` allocation tiers over plain `alloc_ktype`, and
`StoredReach` evidence on the type binding tables
([`bindings.rs`](../../src/machine/core/bindings.rs)) carry the same dead weight.

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


**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — a `Copy`
  digest handle requires a lifetime-free `KType`.

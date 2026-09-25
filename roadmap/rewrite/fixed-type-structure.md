# One statement of the fixed types' structure

**Problem.** Two functions answer whether a written part fills a slot type.
[`admits_part`](../../src/values/admission.rs) reads the slot's node from a
`TypeRegistry`, so it handles every type: a code kind by the code order, a
`List` or `Dict` by its elements, a union by any member. The shape builder's
static check is registry-free, so it asks
[`SlotType::admits_written`](../../src/parse/builtin_shapes.rs) instead, whose
`admits_code` recognises each fixed code container — `LIST_OF_NAME`,
`LIST_OF_DECLARATION`, `DICT_NAME_BLOCK`, `DICT_TYPE_CODE_BLOCK`,
`DICT_NAME_TYPE_CODE` — and the union `TYPE_CODE` by handle equality and
restates their element types by hand. Each fixed composite's structure is so
written in four places: `admits_code`'s arms; `TypeRegistry::seed_constants`
([registry.rs](../../src/type_lattice/registry.rs)), which interns it;
`constants_match_freshly_interned_nodes`
([golden.rs](../../src/type_lattice/tests/golden.rs)), which pins its digest;
and `slot_spelling` ([scope/shape.rs](../../src/scope/shape.rs)), which names
it in a diagnostic and spells every union slot type "a list or dict of names".
A container-typed slot added to the builtin table but not to `admits_code` is
refused `Inadmissible` wherever it is written, with no other symptom.

**Acceptance criteria.**

- Each fixed composite's structure — a container's element, key and value
  types, a union's members — is stated once, and the registry's seeding, the
  golden digest pins, the shape builder's static check and the diagnostic
  naming a slot type all read that statement.
- The shape builder's static check and `admits_part` answer by one rule set:
  no second function restates a container's element rule or the code order's
  admission.
- Adding a container-typed builtin slot is one table entry; a container-typed
  slot the statement lacks fails at build time or in a test, not by refusing
  every program that writes it.

**Directions.**

- *A `const` node table on the handle — open.* `KType::fixed_node`, beside
  the pinned digests in [handle.rs](../../src/type_lattice/handle.rs), names
  the node every fixed handle stands for. `admits_part` takes its node source
  as a parameter — the registry where one is in hand, `fixed_node` in the
  builder — so `SlotType::admits_written` and `admits_code` are deleted, and
  `seed_constants`, the golden test and `slot_spelling` read the same table. A
  union's members sit in canonical order, which the golden test catches, and
  `fixed_node` is `const` if `roles_agree_with_code_types` is to use it. It
  adds lattice API, which lands only with the user's approval.

## Dependencies

**Requires:** none — a refactor of shipped code.

**Unblocks:** none — a leaf.

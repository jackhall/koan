# Region-store tagged and wrapped payloads

Follows the door and pin pattern established by
[region-store-records](region-store-records.md); terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** Both identity-carrying composites box their payloads on the heap:
`KObject::Tagged { value: Rc<KObject>, .. }` and `KObject::Wrapped` via
[`WrappedPayload`](../../src/machine/model/values/kobject.rs), an `Rc<KObject>`
newtype whose `peel` / `hold` constructors `deep_clone` into fresh `Rc`s. Each is a
second ownership channel beside the region for a payload that is an ordinary value.

**Acceptance criteria.**

- `Tagged { value: &'a KObject<'a>, .. }` and `Wrapped { inner: &'a KObject<'a>, .. }`
  — the payload is an ordinary object-family slot; the `WrappedPayload` type is
  deleted.
- Peel (a re-tag collapses one `Wrapped` layer) and hold (a construction preserves
  every layer) are door verbs allocating through the enclosing fold, not payload
  wrapper constructors.
- An escaping tagged or wrapped value pins its birth region by the frame-retention
  hold; `deep_clone` is a pointer copy for both arms.
- No runtime residence walk survives on the tagged or wrapped paths.
- The Miri audit slate is green with region-resident tagged and wrapped values
  exercised.

## Dependencies

**Requires:**

- [Region-store record values](region-store-records.md) — establishes the door and
  pin pattern this conversion follows.

**Unblocks:**

- [Region-store string values](region-store-strings.md)

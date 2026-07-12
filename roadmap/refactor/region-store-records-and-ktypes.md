# Region-store records and resolved KTypes

Hold a record's per-field type memo and an already-resolved `KType` as region
references instead of heap clones, so the resolve/bind/lift hot paths stop re-cloning
types that already live in the region.

**Problem.** `KType` values are region-allocated through
[`alloc_ktype`](../../src/machine/core/arena.rs), but two families still ride the heap
and get cloned on hot paths.

*Record field-type memo.* A record value is
[`KObject::Record(Rc<Record<Held>>, Box<Record<KType>>)`](../../src/machine/model/values/kobject.rs):
its per-field `KType` memo is a `Box<Record<KType>>` that is deep-cloned on every
`.ktype()` call (`kobject.rs` — `KType::Record(field_types.clone())`) and on every
`deep_clone` (`kobject.rs:321`), rather than held as a region reference.

*Re-cloned resolved types.* Resolved `KType`s are repeatedly re-allocated by
`region.alloc_ktype(kt.clone())` — cloning an already-region-allocated `&KType` only to
store the clone in a fresh slot, walking the type's owned `Box`/`Vec`/`Rc` structure
each time. The pattern recurs on the resolver and binder paths:
[`resolve_type_identifier.rs`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
on every resolved bare leaf, plus `val_decl.rs`, `let_binding.rs`, `attr.rs`,
`module_def.rs`, `union.rs`, `newtype_def.rs`, `sig_def.rs`, and `recursive_types.rs`.

So a type that already lives in the region is structurally cloned and re-stored on each
resolution and bind, and a record's field-type memo is boxed on the heap and cloned on
each `.ktype()` / lift, where a region reference would be a cheap copy.

**Acceptance criteria.**

- A record's per-field `KType` memo is reachable as a region reference; `.ktype()` and
  `deep_clone` no longer deep-clone the field-type record.
- On the resolution and bind paths, a `KType` that already lives in the region is
  reused by reference; `region.alloc_ktype(kt.clone())` of an already-region-allocated
  reference is eliminated at the enumerated sites.
- The lift path copies a region reference for unchanged element/key/value types rather
  than cloning them.
- Region-stored record field types obey the same cycle-gate and lifetime discipline as
  other region-allocated `KType`s — no new leak surface.

**Directions.**

- *Record memo: region reference vs intern — open.* Either store the `Record<KType>`
  memo in the region and carry a `&'a Record<KType<'a>>` on `KObject::Record`, or
  intern field-type records. Recommended: region reference, consistent with how
  resolved leaf types already live in the region.
- *De-dup the `alloc_ktype(kt.clone())` sites — decided.* Where the source `&KType` is
  already region-allocated, thread the reference instead of cloning. The resolver-leaf
  redundant allocation is already fixed on the type-name resolution path (see
  [design/typing/elaboration.md](../../design/typing/elaboration.md)); the sites this item
  still owns are the record-memo and lift-path clones.
- *Lift-path clones — open.* The lift path clones element/key/value types even when the
  items are unchanged; decide whether region-stored memos let lift copy a reference.

## Dependencies

An engine-internal memory/hot-path item near
[Content-addressed type identity](../type_memos/type-identity-registry.md). The
`resolve_type_identifier` resolver-leaf redundant allocation it once shared is already
fixed (see [design/typing/elaboration.md](../../design/typing/elaboration.md)). Update
[design/memory-model.md](../../design/memory-model.md) if the region-storage families it
names change.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.

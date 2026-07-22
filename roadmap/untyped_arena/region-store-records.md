# Region-store record values

Pathfinder item — the door and pin pattern chosen here is the one every later
conversion in this project copies; terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** A record value's field substrate rides the heap as an `Rc`:
[`KObject::Record(Rc<Record<Held>>, KType)`](../../src/machine/model/values/kobject.rs)
holds its field cells behind `Rc<Record<Held>>` while koan's memory model homes values
in regions ([design/memory-model.md](../../design/memory-model.md)). The `Rc` is a
second ownership channel beside the region — a record's lifetime is governed by its
refcount, not by the region that should own it: lift shares the record substrate by
refcount rather than by region reference, and the move-in surfaces audit a record's
residence by walking its field cells (`held_resident_in` in `kobject.rs`) instead of
trusting a construction door.

**Acceptance criteria.**

- `KObject::Record` carries `&'a Record<Held<'a>>` — a borrow of a region-allocated
  field substrate — beside the memoized `KType`; no `Rc` in the payload.
- Records are born only through branded doors whose enclosing combinator composes the
  witness naming every operand
  ([design/value-substrates.md § Construction](../../design/value-substrates.md#construction-witnessed-doors-only)).
- The retype path (`stamp_type`, `record_with_type`, the FROM narrowing projection)
  shares the substrate borrow and swaps only the memoized `KType`.
- An escaping record pins its birth region: the consumer takes the producer's
  `Rc<FrameStorage>` hold and mints the record's reach into its own arena;
  `deep_clone` is a pointer copy for the `Record` arm.
- No runtime residence walk survives on the record path — records never route the
  checked move-in tier (`alloc_object_checked` and the `resident_in` walk in
  [`kobject.rs`](../../src/machine/model/values/kobject.rs)), and
  `held_resident_in` has no record caller.
- The Miri audit slate is green (zero UB, zero process-exit leaks) with
  region-resident records exercised.

**Directions.**

- *Substrate immutability — decided* per
  [design/value-substrates.md](../../design/value-substrates.md): no interior field
  writes exist anywhere in the runtime; retype swaps the type handle on a shared
  substrate borrow, so the region-resident substrate needs no mutation story.
- *Door shape at each construction site — open.* Fold placement
  ([`FoldedPlacement`](../../workgraph/src/witnessed.rs) via
  [`FoldingBrand`](../../src/machine/core/arena.rs)) versus the step allocator,
  chosen per construction site; the pathfinder choice here becomes the pattern
  the remaining conversions follow.

## Dependencies

First substrate conversion of the model pinned in
[design/value-substrates.md](../../design/value-substrates.md); the remaining
conversions in this project follow its pattern.

**Requires:** none — first substrate conversion.

**Unblocks:**

- [Region-store list values](region-store-lists.md)
- [Region-store dict values](region-store-dicts.md)
- [Region-store tagged and wrapped payloads](region-store-tagged-wrapped.md)
- [Cost-driven copy at the escape seam](cost-driven-copy.md)

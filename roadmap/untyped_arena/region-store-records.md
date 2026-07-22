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

- `KObject::Record` carries `&'a RecordSubstrate<'a>` — a borrow of a
  region-allocated substrate wrapper holding the field record and its construction
  memos — beside the memoized `KType`; no `Rc` in the payload.
- The substrate memoizes at construction, in the same pass that computes the
  field-type join, its **contains-borrows bit**: set iff some transitive cell is a
  region-borrow leaf (closure, module, non-splice-free expression) or a still-`Rc`
  composite (which carries no memo to consult).
- Records are born only through branded doors whose enclosing combinator composes the
  witness naming every operand
  ([design/value-substrates.md § Construction](../../design/value-substrates.md#construction-witnessed-doors-only)).
- The retype path (`stamp_type`, `record_with_type`, the FROM narrowing projection)
  shares the substrate borrow and swaps only the memoized `KType`.
- At a `Residence::Copied` crossing of the relocation seam, an escaping record is
  **totally copied**: its substrate and every nested substrate (including records met
  inside still-`Rc` spines) rebuild at the destination brand, and the retiring host
  materializes into the minted reach iff a surviving borrow leaf points into that
  host's region — an address-table check per leaf, skipped entirely when the
  contains-borrows bit is clear. A `Residence::Kept` embed pins the host
  unconditionally. `deep_clone` is a pointer copy for the `Record` arm.
- No runtime residence walk survives on the record path — records never route the
  checked move-in tier (`alloc_object_checked` and the `resident_in` walk in
  [`kobject.rs`](../../src/machine/model/values/kobject.rs)), `held_resident_in`
  has no record caller, and the `resident_in` `Record` arm (reached only when a
  record rides inside a still-`Rc` container) is an O(1) address-membership check
  against the region record tables, never a field walk.
- The Miri audit slate is green (zero UB, zero process-exit leaks) with
  region-resident records exercised through both seam verbs (copy and pin).

**Directions.**

- *Substrate immutability — decided* per
  [design/value-substrates.md](../../design/value-substrates.md): no interior field
  writes exist anywhere in the runtime; retype swaps the type handle on a shared
  substrate borrow, so the region-resident substrate needs no mutation story.
- *Door shape — decided.* One record door on
  [`FoldingBrand`](../../src/machine/core/arena.rs): every fold-context site
  allocates through it; a site with no fold in hand mints a zero-dep fold through
  the step allocator. `ExpressionPart::resolve`'s record arm is expected
  unreachable (eager staging routes every record literal through the scheduled
  path) and becomes an `unreachable!` arm after verification.
- *Builtin args record — decided.* Demoted to a transient `&Record<Held>` on
  `BodyCtx`: never a `KObject`, never region-allocated; the `arg_*` accessors read
  it directly and the delivered-evidence placement is deleted.
- *Home-borrow derivation — decided.* A record always borrows its home region (its
  substrate is co-located by the door), so a binding seal sets
  `StoredReach::borrows_into_home` by variant, never by walk; the fold-composed
  carrier bit keeps its machinery meaning, and `Copied`-seam host materialization
  is overridden by the copy pass's exact answer.

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

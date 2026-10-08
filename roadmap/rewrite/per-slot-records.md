# A record per slot

**Problem.** [`BodyShape`](../../src/scope/shape.rs) keeps what it knows about
a slot in parallel tables: `births`, `rhs`, `declarations` and `registrations`
are sorted side tables searched by nineteen hand-written binary searches,
though `Slot` is dense; `declared` and `registered` are parallel `Cell` arrays
while `TypeExpression` embeds its cell; `Statics.narrowings` and
`contributions` run parallel to `candidates` by index, held in step only by
`fix_statics`'s `debug_assert`. Because the facts are not stored together,
readers recover them by search: `birth_site` pointer-scans `nested` for the
site `seal` just resolved; `annotation(slot)` re-walks the statement's roles on
every call and statics calls it four times per member; callers round-trip
`shape.slot(shape.slot_name(slot))` for a position because `names.get` is
private; `enter_body` re-derives the builtin shape three times while
`walk_parts` already holds it. Dispatch keeps `arguments` and `given` as
parallel vectors in `narrow`, and `dropped` reads one where it means the other.
`Native::ALL` in [`builtins.rs`](../../src/dispatch/builtins.rs) mirrors the
enum by hand, so an inserted variant compiles and `Native::of` returns the
wrong native, and `natives_of` zipped with `builtin_shape_types` truncates
silently. `Coerced` in [`coerce.rs`](../../src/knot/module/coerce.rs) stores
its two substitutions as interned `Signature` handles that `across` decodes
back into members at every call. `refusal` and `typing_refusal` are one concept
in two fields, and `Statics.binders` is read only by a test.

**Acceptance criteria.**

- `BodyShape` holds one record per slot (birth, right-hand side, declaration,
  annotation, registration, type cell) indexed directly by `Slot`; no
  hand-written binary search remains in `shape.rs`.
- A call site's narrowing and contributions are read through one
  `use_statics(site)` door, and no `debug_assert` on parallel lengths exists
  because the lengths are one.
- `birth_site`, `annotation` and a `position_of(slot)` are field reads;
  `shape.slot(shape.slot_name(slot))` appears nowhere.
- `enter_body` takes the builtin shape from its caller.
- `narrow` holds one vector of `(argument, given)` pairs, and `dropped` reads
  it.
- `Native` is stored on the overload it belongs to, or a `const` assert pins
  `ALL[i] as usize == i` and the two table lengths.
- `Coerced` holds its substitution members directly.
- One refusal field; `Statics.binders` is deleted or read in production.

**Directions.**

- *Record versus `Keyed<K, V>` — decided.* A per-slot record for the dense
  tables; a `Keyed` sorted-slice newtype only for a table that is genuinely
  sparse over its key, if one remains.
- *Where the record is filled — open.* `seal` fills every field at once, or the
  builder fills a draft record as it walks and `seal` freezes it. Recommended:
  the draft, since the builder holds each fact at the moment it is known.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.

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
  because no table runs parallel to the candidate lists.
- `birth_site`, `annotation` and a `position_of(slot)` are field reads;
  `shape.slot(shape.slot_name(slot))` appears nowhere.
- `enter_body` takes the builtin shape from its caller.
- `narrow` holds one vector of `Given` — each argument's static type beside
  its written names — and `dropped` reads the candidate's copy of it.
- A `const` assert pins `Native::ALL[i] as usize == i`, and another pins each
  dispatched builtin shape's native count to its overload count.
- `across` reads a barrier's two substitutions off its signature handles'
  manifest members without copying them, and a view interns its two
  substitution handles once.
- One refusal field; `Statics.binders` and `BodyShape::binder_type` exist only
  in test builds.

**Directions.**

- *Record versus `Keyed` — decided.* A per-slot record for the dense tables; a
  `Keyed` sorted-slice newtype, over entries that carry their own key, for each
  table that is genuinely sparse over its key: every site-keyed one.
- *Where the record is filled — decided.* The builder fills a draft record per
  slot as it walks, and `seal` freezes it.
- *A barrier's substitutions — decided.* The barrier keeps its two interned
  signature handles and the call reads their manifest members as they are,
  since `substitute_parameters` takes `Parametric` bindings. Holding the members
  in the cell region still rebuilds a `Members` per call, and interning bare
  member tables in the registry duplicates the signature node.
- *Use statics — decided.* `Statics.uses` is one sparse site-keyed table of each
  use's narrowing and contributions, like `named`; the candidate lists are
  untouched.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.

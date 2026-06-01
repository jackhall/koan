# Record structural subtyping and projection

Extend dispatch's specificity lattice with width/depth record subtyping, and
add a projection builtin to coerce among incomparable record arms.

**Problem.** Record and function-parameter types admit only by exact
identity. Once [fn-named-identity](fn-named-identity.md) puts parameter names
in `KType::KFunction` and the [record substrate](../../design/typing/ktype.md#record-fields-and-ktype-hashing) defines
record equality, a value typed `{x :Number, y :Str}` cannot be admitted where
`{x :Number}` is expected, and a function requiring fewer named parameters
cannot sit in a slot that supplies more — even though koan's values are
immutable ([memory-model](../../design/memory-model.md)), which makes a
covariant field-type relation sound. There is also no way for a caller to
*choose* among dispatch arms whose record types overlap but the lattice can't
order.

**Impact.**

- Dispatch's specificity lattice orders records by width and depth: a wider
  record `{x, y}` is more specific than `{x}`, so the most-specific admitting
  arm wins.
- Depth is covariant in field types — sound because koan values are
  immutable; a field re-tagged to a supertype admits.
- Function-parameter records admit contravariantly: a function requiring a
  subset of the supplied named parameters is usable where one requiring more
  is expected.
- A `FROM` projection builtin lets a caller narrow a record's type to pick a
  specific arm when two arms are *incomparable* (neither more specific),
  which the lattice alone can't disambiguate.

**Directions.**

- *Width / depth admission — decided.* Records admit by width (drop fields)
  and depth (covariant field types). Permutation is already order-blind per
  the substrate.
- *Function subtyping — decided: contravariant in the parameter record,
  covariant in the return.* The argument record follows the substrate's
  subtyping; the return follows existing `KType` admission.
- *Lattice specificity — decided.* A record with a superset of fields is
  strictly more specific, mirroring the one-directional `UserType` ⊏
  `AnyUserType` ordering already in
  [ktype.rs](../../src/machine/model/types/ktype.rs). Incomparable arms
  (`{x, y}` vs `{x, z}`) remain a dispatch ambiguity resolved by projection,
  not by the lattice.
- *Projection surface — open.* The narrowing builtin reads as
  `([x, y] FROM r)` — its first argument is a `List` of identifiers (the
  fields to keep). Surface keyword (`FROM`) and whether the identifier list
  is a literal-only position are open.
- *Projection is type-computing — decided.* Its result type is derived from
  the literal identifier list, so it routes like the dispatcher-only `_OF`
  ops ([scheduler.md](../../design/typing/scheduler.md)), not as an ordinary
  value builtin.
- *Projection semantics — decided: re-typing, not erasing.* Projection
  `Rc`-shares the backing record and narrows the carried field-type map — the
  same move `stamp_type`
  ([kobject.rs](../../src/machine/model/values/kobject.rs)) makes for `List` /
  `Dict`. Dropped fields stay physically present but invisible through the
  narrowed type, consistent with dispatch trusting the carried type rather
  than walking contents.

## Dependencies

**Requires:**

- [FN/FUNCTOR named identity](fn-named-identity.md) — contravariant function
  subtyping needs parameter names in the `KType`.

**Unblocks:**

- [Structural KFunction admission across deferred parameter and return slots](kfunction-deferred-ret-precision.md)
  — function-type admission becomes structural record subtyping, superseding
  the strict-`==` comparison that item assumed.

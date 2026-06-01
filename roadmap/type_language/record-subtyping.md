# Standalone record type and projection

Add a structural record type — a `KType::Record` variant and an anonymous
record *value* with surface syntax — with width/depth subtyping on record
values, plus a `FROM` projection builtin to coerce among incomparable record
arms.

**Problem.** Koan has a record *substrate* but no record *type*. The
[`Record<V>`](../../src/machine/model/types/record.rs) substrate backs struct
fields and `KFunction` / `KFunctor` parameter identity (see the
[record substrate](../../design/typing/ktype.md#record-fields-and-ktype-hashing)),
and function admission already runs width/depth subtyping over parameter
records (see [ktype.md § Variance](../../design/typing/ktype.md#variance)). But
there is no first-class structural record:

- No `KType::Record` variant in
  [ktype.rs](../../src/machine/model/types/ktype.rs). Records exist only as the
  internal `params` field of `KFunction` / `KFunctor` and as a struct schema's
  fields — never as a type a slot can name on its own.
- Structs are **nominal**, not structural: a `UserType { kind: Struct,
  scope_id, name }` admits by per-declaration `scope_id` identity, and its
  `Rc<Record<KType>>` payload is ignored by equality (see the `UserTypeKind`
  payload-ignoring `PartialEq`). Two structurally identical structs declared
  separately are distinct types.
- No anonymous record *value*: `Record<KObject>` is named in the substrate's
  generic but never instantiated, and there is no surface syntax for an
  anonymous `{x: 1, y: "a"}` record literal.

So a value cannot be typed `{x :Number, y :Str}` directly, a wider record
cannot be admitted where a narrower one is expected on its own type, and there
is no way for a caller to *choose* among dispatch arms whose record types
overlap but the lattice can't order.

**Impact.**

- A slot can name a structural record type (`{x :Number, y :Str}`) backed by a
  `KType::Record` variant, distinct from any nominal struct.
- An anonymous record value with surface syntax instantiates the substrate's
  `Record<KObject>` value level, completing the type/value pairing the
  substrate's generic anticipates.
- Dispatch's specificity lattice orders record *values* by width and depth: a
  wider record `{x, y}` is more specific than `{x}`, so the most-specific
  admitting arm wins; depth is covariant in field types, sound because koan
  values are immutable ([memory-model](../../design/memory-model.md)).
- A `FROM` projection builtin lets a caller narrow a record's type to pick a
  specific arm when two arms are *incomparable* (neither more specific), which
  the lattice alone can't disambiguate.

**Directions.**

- *`KType::Record` variant — open.* Add a structural record variant carrying a
  `Record<KType>`, distinct from the nominal `UserType { kind: Struct }`
  carrier. Whether the struct schema's field record collapses onto the same
  variant or stays nominal-only is the open choice. *Recommended: keep structs
  nominal; the new variant is structural-only.*
- *Anonymous record value + surface syntax — open.* Instantiate
  `Record<KObject>` as a first-class value and give it a literal surface form.
  The literal syntax (a braced `{x: 1, y: "a"}` form vs. a keyworded builtin)
  is open.
- *Width / depth admission — decided.* Record values admit by width (drop
  fields) and depth (covariant field types). Permutation is already order-blind
  per the substrate.
- *Lattice specificity — decided.* A record with a superset of fields is
  strictly more specific, mirroring the one-directional `UserType` ⊏
  `AnyUserType` ordering already in
  [ktype.rs](../../src/machine/model/types/ktype.rs). Incomparable arms
  (`{x, y}` vs `{x, z}`) remain a dispatch ambiguity resolved by projection,
  not by the lattice.
- *Projection surface — open.* The narrowing builtin reads as
  `([x, y] FROM r)` — its first argument is a `List` of identifiers (the fields
  to keep). Surface keyword (`FROM`) and whether the identifier list is a
  literal-only position are open.
- *Projection is type-computing — decided.* Its result type is derived from the
  literal identifier list, so it routes like the dispatcher-only `_OF` ops
  ([scheduler.md](../../design/typing/scheduler.md)), not as an ordinary value
  builtin.
- *Projection semantics — decided: re-typing, not erasing.* Projection
  `Rc`-shares the backing record and narrows the carried field-type map — the
  same move `stamp_type`
  ([kobject.rs](../../src/machine/model/values/kobject.rs)) makes for `List` /
  `Dict`. Dropped fields stay physically present but invisible through the
  narrowed type, consistent with dispatch trusting the carried type rather than
  walking contents.

## Dependencies

**Requires:**

None — the [record substrate](../../design/typing/ktype.md#record-fields-and-ktype-hashing)
this builds on has shipped, and the function-subtyping slice that exercised
width/depth subtyping over parameter records has landed (see
[ktype.md § Variance](../../design/typing/ktype.md#variance)).

**Unblocks:**

- [Structural KFunction admission across deferred parameter and return slots](kfunction-deferred-ret-precision.md)
  — a standalone `KType::Record` gives the deferred-carrier precision work a
  structural record to record per-call elaboration intent into.
- [Argument-binding unification](argument-binding-unification.md) — the
  anonymous record value is the runtime carrier that lets a call's arguments
  install as one record block, making function-param width-drop native at the
  invoke path rather than only at the type level.

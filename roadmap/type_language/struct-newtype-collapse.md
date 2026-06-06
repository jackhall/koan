# Collapse `STRUCT` into a record-repr `NEWTYPE`

Phase one of shrinking `NominalKind` toward a single `Newtype` primitive: a struct
becomes a nominal identity over a `KType::Record` repr, retiring the separate `Struct`
carrier, schema, and kind.

**Problem.** `STRUCT` and `NEWTYPE` are two nominal declarators encoding one concept —
a nominal identity over a structural shape. A struct's schema is *already* a record
(`NominalSchema::Struct(Record<KType>)`, `src/machine/model/types/recursive_set.rs`),
and a newtype is a nominal identity over a transparent repr
(`NominalSchema::Newtype(Box<KType>)`). The only things separating a struct from a
newtype whose repr is a record are the value carrier
(`KObject::Struct { set, index, fields }` vs `KObject::Wrapped { inner, type_id }`,
`src/machine/model/values/kobject.rs`), the construction surface
(`(Point {x = 1, y = 2})` named record-literal body vs `(Distance 3.0)` positional
wrap), and a redundant `NominalKind::Struct` / `NominalSchema::Struct` /
`ProjectedSchema::Struct` triple plus the struct arm of `apply_constructor`
(`src/machine/execute/dispatch/apply_callable.rs`) and `dispatch_construct_struct`.

**Impact.**

- *One nominal product primitive.* A struct is a `NominalKind::Newtype` member whose
  repr is a `KType::Record`, carried as `Wrapped { inner: KObject::Record, type_id }`.
  The `Struct` carrier, schema variant, projected-schema variant, kind, and dedicated
  constructor path all retire.
- *Record-repr newtype construction reaches struct parity.* `(Point {x = 1, y = 2})`
  builds the record and wraps it; `(Point r)` wraps an existing record value. Both ride
  the newtype construction path, which branches on repr shape (record vs scalar) rather
  than on a separate kind.
- *Constructors reified as first-class function values.* `(<Type> CONSTRUCTOR)`
  evaluates to a `KObject::KFunction` typed `:(FN (fields…) -> <Type>)`, so a
  constructor binds anywhere a function does — a higher-order argument, an `FN`-typed
  slot, a `LET`. With one constructible nominal kind, the reification is uniform.
- *`.x` access through the unified carrier.* Field access on an ex-struct extends ATTR's
  existing `Wrapped` fall-through (already handling newtype-over-struct) to
  newtype-over-record.

**Directions.**

- *`STRUCT` declarator retired — decided.* `STRUCT Name = (fields)` goes away; the
  spelling is `NEWTYPE Name = :{fields}`. koan has no users, so existing declarations
  migrate freely.
- *Construction spellings — decided.* A record-repr newtype constructs with the named
  record-literal body `(Point {x = 1, y = 2})` (build + wrap) and the positional
  `(Point r)` (wrap an existing record), both in koan's `(verb arg)` form. Scalar reprs
  keep `(Distance 3.0)`.
- *Value carrier — decided (full collapse).* `KObject::Struct` is removed; ex-structs are
  `Wrapped { inner: KObject::Record, type_id }`. Field access pays one peel indirection
  through the wrapper — accepted for the single-carrier simplification.
- *`NominalKind::Struct` and the `:Struct` wildcard — decided (drop both).* The kind is
  removed, so all record/scalar nominals are `Newtype`. The `:Struct` wildcard is dropped
  rather than re-pointed: "record-shaped vs scalar-wrapper" is a *repr-shape* distinction
  (`Newtype(Record)` vs `Newtype(scalar)`), which the eagerly-recorded `NominalKind` axis
  cannot encode. If "match any record-repr nominal" is ever wanted, it belongs on a future
  repr-shape wildcard, not the kind axis.
- *Recursion through a record repr — decided (already supported).* `seal_recursive_refs`
  / `resolve_set_locals` already descend into `KType::Record` field types
  (`src/machine/model/types/recursive_set.rs`), so `NEWTYPE Node = :{value :Number,
  next :Node}` threads the `next` back-edge to a `SetLocal` exactly as a self-recursive
  struct does today.
- *Scope — product side only, decided.* This item dissolves only `Struct` into `Newtype`.
  The sum-side `Tagged` collapse is a separate sequel, downstream of [anonymous structural
  unions](anonymous-unions.md) and [tagged-union variants as dispatchable
  types](tagged-variant-types.md): a tagged union decomposes into the anonymous-union
  *join* of per-variant `Newtype`s, each variant's nominal identity replacing its tag
  string. That sequel is a different mechanism than this one — an untagged union can't
  carry the tag discriminant — and recursive unions there still need the union name to
  thread a `SetLocal` back-edge, the same nominal anchor a record-repr `Newtype` keeps.

## Dependencies

**Requires:**

- [Type-only nominal identities](../../design/typing/user-types.md) — the shipped
  `NominalKind` / `NominalSchema` / `Wrapped` substrate and the record-sigil `:{…}` →
  `KType::Record` resolution this work collapses.

**Unblocks:**

- [Tagged-union variants as dispatchable types](tagged-variant-types.md) — the sum-side
  sequel of the `NominalKind` → `Newtype` collapse: it dissolves `Tagged` by making each
  variant a `Newtype` over its payload, building on the product-side primitive this item
  establishes.

This item is **phase one** of shrinking `NominalKind` from four kinds toward
`{Newtype, TypeConstructor}` — it dissolves `Struct` into `Newtype`. The sum-side
`Tagged` sequel and its prerequisites are framed in the Scope direction above.

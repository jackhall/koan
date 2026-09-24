# Value, type and code tops

A universal top over three disjoint families, so admission alone keeps a
program's values, types and code apart.

**Problem.** `Any` is the one top of the
[type lattice](../../src/type_lattice/README.md), kinds and code included:
[`is_subtype_of`](../../src/type_lattice/order.rs) answers true for any pair
whose right side is `Any`. So no slot can admit every value without also
admitting a type value and a quote, and a slot can keep types out only through
the name its argument binds to: a value name never holds a type, so a keyworded
call that hands a type to a value slot is admitted and then refused at bind,
rather than falling through to another overload. A kind slot (`OfKind`) is the
one slot already confined to a family. The raw-part types a quote or a raw
part is typed by — `Identifier`, `NameToken`, `TypeNameToken`, `KExpression`,
`SigiledTypeExpr`, `RecordType` — sit under `Any` beside `Number` and `Str`.

**Acceptance criteria.**

- `Any` is the top of every type. Below it lie three family tops: `AnyValue`
  over every value type, `AnyType` — the kind lattice's top,
  `OfKind(AnyType)` — over every kind, and `AnyCode` over the raw-part types.
  No type lies below two family tops.
- A union holding all three family tops is `Any`.
- A type value satisfies only a kind and `Any`, a value no kind, and a quote
  only `AnyCode`, the types below it, and `Any`.
- A lowercase name holds a value, a type or code: an `Any` parameter binds a
  type argument or a quote without a refusal at bind. A capitalized name holds
  only a type.
- Joining types from different families yields their canonical union, so a
  list literal mixing a value and a type is a list whose element type is the
  union of the value's type and the type's kind.
- The default bound of a `FOR ALL` parameter is `Any`.
- `Never` stays the one bottom, below every top.
- The lattice's order, join and meet properties hold over the four tops in the
  lattice's property tests.

**Directions.**

- *Four tops — decided.* Admission is the lattice's job alone: a slot meant for
  one family names that family's top, and `Any` admits all three.
- *A name's case decides how it resolves, not what it holds — decided.* A
  capitalized name resolves and is elaborated where the shape is built, so only
  it stands in type position. A lowercase name is read at run time and may hold
  a type or code as data, which it can pass, print and compare but never use in
  type position. So introspection costs elaboration nothing.
- *Pair tops are unions — decided.* `AnyValue | AnyType` is the top of values
  and types together. A named node for it would be a second node for one type,
  each below the other, which breaks the order's antisymmetry.
- *The default `FOR ALL` bound — decided.* `Any`, so a generic function carries
  types and code unless its author narrows the bound, and a library needs no
  second copy for code.
- *Capitalized names hold only types — decided.* Code has a name class of its
  own, marked by a sigil ([code as values](code-values.md)), so elaboration
  never meets code under a type name.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — selection keeps the families apart by admission.
- [Code as values](code-values.md) — the code kinds lie under `AnyCode`.

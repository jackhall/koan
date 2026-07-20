# Record fields, hashing, and limits

The `Record<KType>` field substrate and `KType` hashing, known limitations, and
open work. Part of the [`KType` reference](README.md).

## Record fields and `KType` hashing

A record schema's fields are a [`Record<V>`](../../../src/machine/model/types/record.rs) —
an ordered identifier-keyed map, generic over its value, so the type level stores
`Record<KType>` and a value level can later store `Record<KObject>`.
A record-repr member's [`NominalSchema::NewType`](../../../src/machine/model/types/recursive_set.rs)
wraps a `KType::Record(Record<KType>)`, carrying the field record by value; the `NEWTYPE`
elaborator wraps the parser's declaration-ordered `(name, KType)` pairs into a `Record` once,
at the [`finalize_nominal_member`](../../../src/machine/model/types/recursive_set.rs)
boundary, and fills the member's schema cell.

The same `Record<KType>` substrate backs `KFunction` parameter
identity: the variant stores its parameters as `params: Record<KType>`
(`(name → type)`), built by `finalize_carrier` in
[`parameterized_types.rs`](../../../src/builtins/parameterized_types.rs) from the
shared field-list parser NEWTYPE / UNION use. A function-typed slot is thus
identified by its parameter names and types order-blind — `:(FN (x :Number,
y :Str) -> Bool)` equals `:(FN (y :Str, x :Number) -> Bool)`. Function
admission compares the two records under width-drop subtyping (see
[Variance](parameterization-and-variance.md#variance)): a value that requires a parameter the slot doesn't
declare is a non-match, while extra *slot* parameters the value doesn't declare
are fine — they arrive unbound under call-by-name. `KType::join` reuses the
record join for both arms.

The shape has two defining properties:

- **Insertion order is preserved** for rendering and positional construction
  (`Record::iter` walks declaration order), but **equality ignores it**:
  `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same record. The
  order-blind `PartialEq` is `IndexMap`'s, forwarded directly. Names are unique
  within a record — a structural property `IndexMap` keys carry for free, and one
  `parse_pair_list` already enforces by rejecting a duplicate field name.
- **Hashing agrees with that order-blind equality**: a commutative fold
  (`wrapping_add`) over a per-field `mix(hash(name), hash(value))`. The `mix` binds
  name to value before the fold, so `{x: Number}` and `{y: Number}` hash apart; the
  symmetric accumulator makes the result independent of field order. Wrapping-add
  rather than XOR, which would cancel on a duplicate.

`Record<V>: Hash` needs `V: Hash`, so `KType` implements `Hash`, kept consistent with
its hand-written `PartialEq` arm-for-arm: the discriminant leads (so distinct variants
never alias and the unit variants need no further mixing), then each compound arm
hashes exactly the fields its `PartialEq` arm compares. The pointer-identity
variants hash their stable identity key — `Module` hashes `scope_id()`,
`AbstractType` hashes its `source.scope_id()`, `Signature` hashes `sig_id()`,
`SetRef` hashes `(Rc::as_ptr(set), index)` and `RecursiveGroup` hashes
`Rc::as_ptr(set)` — never descending the (possibly cyclic) member schema, so
hashing terminates and agrees arm-for-arm with the pointer-keyed `PartialEq`.

`KType::DeferredReturn(DeferredReturnSurface)` is a confined hashable leaf: it
holds the type-language shadow of a per-call-elaborated function return —
`TypeExpr(TypeName)` for parser-preserved leaf forms, `Expression(String)` for
the canonical `summarize()` render of a parens-form return (the live
`KExpression` impls neither `Eq` nor `Hash`). It hashes and compares by that
shadow, so two functions differing only in their deferred returns are distinct
structural types. The variant is valid *only* inside a synthesized
`KFunction` `ret` box that `function_value_ktype` builds; no runtime
value's `ktype()` returns it free-standing, and it admits nothing on its own
(`accepts_part` is `false`).

The same `Record<V>` substrate also backs the first-class structural record type
`KType::Record(Record<KType>)` and its value `KObject::Record(Record<KObject>, …)`
(surface `{x = 1, y = "a"}`). The dict carrier (`KType::Dict`, `KObject::Dict`) stays
a sibling: records restrict keys to identifiers and admit heterogeneous per-field
types, while dicts admit arbitrary value keys and one homogeneous value type. The two
never share a key representation, and the value surfaces disambiguate at parse time —
a brace literal with `=` pairs (`{x = 1}`) is a record, with `:` pairs (`{k: v}`) a
dict. Record field names are unique by *parse* rule, not only by the `IndexMap` key
invariant: a repeated name in a record literal is a parse error, while a dict may repeat
a key (last wins), since dict keys are runtime-evaluated value expressions rather than a
static shape (see [type-language-via-dispatch.md § Record-type sigil](../type-language-via-dispatch.md#record-type-sigil)).

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `ReturnContract` carrier is replaced at TCO
  time. A nested `MATCH` / `TRY` arm whose body tail-calls a function is checked
  against the callee's contract, not the arm's `-> :T`.
- **Value-returning builtins are not runtime-checked.** They return through
  a `Done` value with no slot frame, so the runtime check has nowhere to
  attach; their declared return types are honest but unenforced. `MATCH` / `TRY`
  are the exception — they return through an `Action::Tail` carrying a
  `ReturnContract::Arm`, so their `-> :T` is enforced.
The two-phase execution work in [open-work.md](../open-work.md) closes both
uniformly.

## Open work

None tracked.

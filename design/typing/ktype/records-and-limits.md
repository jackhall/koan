# Record fields, hashing, and limits

The `Record<KType>` field substrate and `KType` hashing, known limitations, and
open work. Part of the [`KType` reference](README.md).

## Record fields and `KType` hashing

A record schema's fields are a [`Record<V>`](../../../src/machine/model/types/record.rs) —
an ordered name-keyed map, generic over its value, so the type level stores
`Record<KType>` and a value level can later store `Record<KObject>`.
A record-repr member's [`NodeSchema::NewType`](../../../src/machine/model/types/node.rs)
holds the handle of an interned `Record` node (`TypeNode::Record { fields: Record<KType> }`);
the `NEWTYPE` elaborator wraps the parser's declaration-ordered `(name, KType)` pairs into a
`Record`, interns the record node, and seals the member's schema against the group window
([`recursive_group_window.rs`](../../../src/machine/model/types/recursive_group_window.rs)).

The same `Record<KType>` substrate backs `KFunction` parameter
identity: the variant stores its parameters as `params: Record<KType>`
(`(name → type)`). An `:(FN :{…} -> …)` type takes that record whole — its
parameter list *is* a record type, elaborated before `FN` dispatches, and
[`parameterized_types.rs`](../../../src/builtins/parameterized_types.rs)'s body
unwraps the resolved `TypeNode::Record` and interns the function type. A
function-typed slot is thus
identified by its parameter names and types order-blind — `:(FN :{x :Number,
y :Str} -> Bool)` equals `:(FN :{y :Str, x :Number} -> Bool)`. Function
admission compares the two records under width-drop subtyping (see
[Variance](parameterization-and-variance.md#variance)): a value that requires a parameter the slot doesn't
declare is a non-match, while extra *slot* parameters the value doesn't declare
are fine — they arrive unbound under call-by-name. `TypeRegistry::join` and its dual `meet`
share one name-keyed pointwise combinator over that record, each passing the other down for the
contravariant parameter position.

The shape has three defining properties:

- **Keys are [`BinderSymbol`s](../../label-interning.md), not text.** The backing is a plain
  `Vec<(BinderSymbol, V)>` — one allocation, no index table — so a lookup is a linear `u128`
  compare over a handful of fields and no field name is ever copied into a record.
  Rendering resolves the text back through the run's label interner. A key carries the
  binding class its own declaration established (`x` a value token, `Elt` a Type token), so a
  schema hands that class back past the intern boundary instead of erasing it — **identity
  reads the key's symbol bits alone**: `PartialEq`, `Hash` and the type digest all go through
  `key.symbol()` and never see the variant tag. Probe doors (`Record::get`,
  `get_index_of`, `remove`) therefore take a bare `Symbol` — a reference is not a
  declaration and has no class to assert — and `Record::get_key_value` is the recovery
  door that hands a stored key back with its class, witnessed because insertion required a
  classified key ([label-interning.md § Classified label vocabulary](../../label-interning.md#classified-label-vocabulary)).
- **Insertion order is preserved** for rendering and positional construction
  (`Record::iter` walks declaration order), but **equality ignores it**:
  `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same record. `PartialEq`
  matches every field of one against the other at equal length, which is set equality
  because names are unique — a `parse_pair_list` rejects a duplicate field name
  upstream, and `Record::insert` is last-wins if one ever arrived anyway.
- **Hashing agrees with that order-blind equality**: a commutative fold
  (`wrapping_add`) over a per-field `mix(hash(symbol), hash(value))`. The `mix` binds
  name to value before the fold, so `{x: Number}` and `{y: Number}` hash apart; the
  symmetric accumulator makes the result independent of field order. Wrapping-add
  rather than XOR, which would cancel on a duplicate.

`Record<V>: Hash` needs `V: Hash`, and `KType` is a `Copy` wrapper around its content
digest, so `Hash`, `Eq`, and `Ord` all derive on that one digest — a record node's
field record is hashed by the order-blind fold above only when the node's *digest* is
computed at intern time, never on every `KType` compare. Because a handle is `Copy` and
opaque, hashing a `KType` never descends the (possibly cyclic) member schema; it copies
sixteen bytes. Equality and hashing agree unconditionally, since both read the same digest.

`KType::DeferredReturn(DeferredReturnSurface)` is a confined hashable leaf: it
holds the type-language shadow of a per-call-elaborated function return —
`TypeExpr(TypeName)` for parser-preserved leaf forms, `Expression(String)` for
the canonical `summarize()` render of a parens-form return (the live
`KExpression` impls neither `Eq` nor `Hash`). It hashes and compares by that
shadow, so two functions differing only in their deferred returns are distinct
structural types. The node is valid *only* as the `ret` handle of a synthesized
`KFunction` node that `function_value_ktype` interns; no runtime
value's `ktype()` returns it free-standing, and it admits nothing on its own
(`accepts_part` is `false`).

The same `Record<V>` substrate also backs the first-class structural record type
node `TypeNode::Record { fields: Record<KType> }`, whose values are
`KObject::Record(&RecordSubstrate, KType)` (surface `{x = 1, y = "a"}`) — the value
side lays its cells out symbol-sorted behind a region-hosted index rather than
carrying a `Record` of its own ([value-substrates.md](../../value-substrates.md)).
The dict carrier (`KType::Dict`, `KObject::Dict`) stays
a sibling: records restrict keys to identifiers and admit heterogeneous per-field
types, while dicts admit arbitrary value keys and one homogeneous value type. The two
never share a key representation, and the value surfaces disambiguate at parse time —
a brace literal with `=` pairs (`{x = 1}`) is a record, with `:` pairs (`{k: v}`) a
dict. Record field names are unique by *parse* rule, not only by `Record`'s last-wins
insert: a repeated name in a record literal is a parse error, while a dict may repeat
a key (last wins), since dict keys are runtime-evaluated value expressions rather than a
static shape (see [type-language-via-dispatch.md § Record-type sigil](../type-language-via-dispatch.md#record-type-sigil)).

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `ReturnContract` carrier is replaced at TCO
  time. A nested `MATCH` / `TRY` arm whose body tail-calls a function is checked
  against the callee's contract, not the arm's `-> :T`.
- **Value-returning builtins are not runtime-checked.** They return a `Done`
  value carrying no `ReturnContract`, so no obligation is sealed and the runtime
  check — which fires on the obligation's presence at the Done boundary — has
  nothing to read; their declared return types are honest but unenforced.
  `MATCH` / `TRY` are the exception: they return through an `Action::Tail`
  carrying a `ReturnContract::Arm`, so their `-> :T` is enforced wherever the arm
  runs, top level included.
The two-phase execution work in [open-work.md](../open-work.md) closes both
uniformly.

## Open work

None tracked.

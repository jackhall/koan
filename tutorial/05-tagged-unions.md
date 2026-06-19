# Tagged unions

A tagged union is a type whose values are one of several named alternatives.
Each value carries a **tag** saying which alternative it is, plus a payload.
Unions are how you model "one of these shapes" — present or absent, success or
failure, a node or a leaf.

## Declaring a union

`UNION <Name> = (<variants>)` declares the type. Each variant is a tag paired
with its payload type:

```koan
UNION Maybe = (Some :Number None :Null)
PRINT (Maybe (Some 42))
PRINT (Maybe (None null))
```

```text
Some(42)
None(null)
```

A tag is a **type-name** token — capitalized with at least one lowercase letter
(`Some`, `None`), never a lone capital. The variant list pairs each tag with a
payload type, separated by whitespace or commas. You construct a value by
calling the union with a `(Tag payload)` pair, as above. A union value prints as
its bare tag and payload (`Some(42)`), since the tag already identifies it.

Each `UNION` declaration mints its own distinct type, even if another union has
the same variant names. A few shapes are rejected:

- a lowercase tag (`some`) — tags must be capitalized type names;
- an empty variant list;
- two variants with the same tag.

## Aliasing a union

A union type can be given another name, as long as that name is a type name
(capitalized with a lowercase letter). The alias constructs through the same
type:

```koan
UNION Maybe = (Some :Number None :Null)
LET Option = Maybe
PRINT (Option (Some 7))
```

```text
Some(7)
```

Binding a type to a *lowercase* name is rejected — types live in the type
namespace:

```koan
UNION Maybe = (Some :Number None :Null)
LET maybe = Maybe
```

```text
error: shape error: LET binder `maybe` is value-classified but the bound value is a type (a type-language carrier); rebind under a Type-classified identifier instead (uppercase-leading plus at least one lowercase letter, e.g. `Maybe`)
```

## Dispatching on a variant

Each variant is itself a type you can name with the union-qualified sigil
`:(<Union> <Tag>)`. A slot typed `:(Maybe Some)` admits only `Some` values,
while a slot typed `:Maybe` admits any variant. That lets two functions sharing
a keyword dispatch on which variant they're handed:

```koan
UNION Maybe = (Some :Number None :Null)
FN (DESCRIBE x :(Maybe Some)) -> Str = ("has a value")
FN (DESCRIBE x :(Maybe None)) -> Str = ("empty")
PRINT (DESCRIBE (Maybe (Some 1)))
PRINT (DESCRIBE (Maybe (None null)))
```

```text
has a value
empty
```

Variant dispatch like this is one way to branch on a union. The other — and the
one that binds the payload so you can use it — is [pattern matching](06-pattern-matching.md),
next.

Next: [Pattern matching](06-pattern-matching.md).

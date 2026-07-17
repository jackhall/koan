# Newtypes

`NEWTYPE` mints a fresh type identity. You've already seen its record form for
[records](07-records.md); this chapter covers its other form, where a newtype
wraps an existing type to make a distinct one:

```koan
NEWTYPE Distance = Number
LET d = (Distance 3.0)
PRINT d
```

```text
Distance(3)
```

A `Distance` is *represented* as a `Number`, but it is a different type. You
construct one by calling the type with a value of its representation.

## Distinct at dispatch

The point of a scalar newtype is that it doesn't interchange with its
representation. A slot typed `Number` rejects a `Distance`, and a slot typed
`Distance` rejects a plain `Number`, so the two route to different overloads:

```koan
NEWTYPE Distance = Number
FN (SHOW x :Number) -> Str = ("a plain number")
FN (SHOW x :Distance) -> Str = ("a distance")
PRINT (SHOW 3.0)
PRINT (SHOW (Distance 3.0))
```

```text
a plain number
a distance
```

This is what records can't give you ergonomically: pairs of types that share a
representation but mean different things — `Distance` and `Duration`, `UserId`
and `PostId` — without wrapping each in a single-field record. Constructing one
from the wrong representation is an error:

```koan
NEWTYPE Distance = Number
Distance "far"
```

```text
error: type mismatch for argument 'value': expected Number, got Str
```

## Wrapping other types

A newtype's representation can be any type, including a record type or another
newtype. When it wraps a record, field access *falls through* the wrapper:

```koan
NEWTYPE Point = :{x :Number, y :Number}
NEWTYPE Boxed = Point
LET p = (Point {x = 1, y = 2})
LET b = (Boxed p)
PRINT b.x
```

```text
1
```

The fall-through is transparent, except that a missing-field error names the
wrapper you accessed, not the type underneath:

```koan
NEWTYPE Point = :{x :Number, y :Number}
NEWTYPE Boxed = Point
LET p = (Point {x = 1, y = 2})
LET b = (Boxed p)
b.z
```

```text
error: shape error: `Boxed` has no field `z`
```

Wrapping a newtype in another newtype collapses to a single layer — however many
times you wrap a value, the representation sits exactly one wrapper deep.

## Mutually recursive types

A type may refer to itself directly — a union whose variant payload is the union
itself, for instance. But two *different* types that refer to each other can't
be declared one after another, because the first can't name the second before
it exists. `RECURSIVE TYPES` declares such a group together, so every member is
in scope for every other:

```koan
RECURSIVE TYPES Listy = (
  NEWTYPE Cell = :{head :Number, tail :Rest}
  NEWTYPE Rest = :{next :(Cell | Null)}
)
LET empty = (Rest {next = null})
LET one = (Cell {head = 1, tail = empty})
LET chain = (Rest {next = one})
PRINT chain
```

```text
Rest({next = Cell({head = 1, tail = Rest({next = null})})})
```

Here `Cell` names `Rest` and `Rest` names `Cell` — each definition mentions the
other. The body holds only type declarations (two `NEWTYPE`s here), one per line,
each indented under the opening line. The group name (`Listy`) must differ from
every member name.

Next: [Errors](09-errors.md).

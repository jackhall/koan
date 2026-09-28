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
EXPR #(SHOW x :Number) -> Str = #("a plain number")
EXPR #(SHOW x :Distance) -> Str = #("a distance")
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
error: Distance cannot wrap Str: its representation is Number
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
error: Boxed has no field z
```

## Naming a field's type

`b.x` reads a field off a *value*. You can also ask a record-repr newtype for the
*type* it declared a field with, by writing the projection under the type sigil
`:(…)`. That is useful when you want a slot to track a declaration instead of
restating it:

```koan
NEWTYPE Point = :{x :Number, y :Str}
EXPR #(LABEL v :(Point.y)) -> Str = #(v)
PRINT (LABEL "north")
```

```text
north
```

`:(Point.y)` is the field's declared `Str`, so the slot admits exactly what a slot
spelled `:Str` admits. Change the declaration and the slot follows. The read chains
where a field is itself record-shaped — `:(Outer.inner.x)` — and works through an
alias of the type name. Like field access on a value, it falls through a wrapping
newtype: with `NEWTYPE Boxed = Point`, `:(Boxed.x)` is `Point`'s `x`.

The sigil is optional, though: reading a field off a type, as off any other
value, gives the type its record declares the field with. So `v :Point.y` is the
same slot, and the read runs anywhere a value does:

```koan
NEWTYPE Point = :{x :Number, y :Str}
PRINT Point.y
```

```text
Str
```

Uppercase members read the same way: `Maybe.Some` names a union variant bare, and
`:(Maybe.Some)` names the same thing.

## Mutually recursive types

A type may refer to itself directly — a union whose variant payload is the union
itself, for instance — and two different types may refer to each other. A body
announces its type declarations before it runs any of them, so every one is in
scope for every other regardless of order:

```koan
NEWTYPE Cell = :{head :Number, tail :Rest}
NEWTYPE Rest = :{next :(Cell | Null)}
LET empty = (Rest {next = null})
LET one = (Cell {head = 1, tail = empty})
LET chain = (Rest {next = one})
PRINT chain
```

```text
Rest({next = Cell({head = 1, tail = Rest({next = null})})})
```

Here `Cell` names `Rest` before `Rest` is declared, and `Rest` names `Cell` — each
definition mentions the other.

Next: [Errors](09-errors.md).

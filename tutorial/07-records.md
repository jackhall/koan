# Records

A record is a value with named, typed fields — the product type to the tagged
union's sum type. You declare a named record type with `NEWTYPE`, giving it a
record representation:

```koan
NEWTYPE Point = :{x :Number, y :Number}
LET p = (Point {x = 3, y = 4})
PRINT p
```

```text
Point({x = 3, y = 4})
```

`NEWTYPE <Name> = :{<fields>}` declares the type; each field is `name :Type`
inside the `:{...}` schema. You construct a value by calling the type with a
**record literal** — braces with `field = value` pairs (note the `=`, which is
what distinguishes a record literal from a [dictionary](02-values-and-types.md#dictionaries)
literal's `:`). Fields are matched by name, so their order is free:

```koan
NEWTYPE Point = :{x :Number, y :Number}
PRINT (Point {y = 4, x = 3})
```

```text
Point({y = 4, x = 3})
```

Bare identifiers on the value side resolve through scope like anywhere else, so
you can build a record from existing names:

```koan
NEWTYPE Point = :{x :Number, y :Number}
LET vx = 7
LET vy = 11
PRINT (Point {x = vx, y = vy})
```

```text
Point({x = 7, y = 11})
```

## Reading fields

The `.` operator reads a field off a record value, and chains:

```koan
NEWTYPE Point = :{x :Number, y :Number}
NEWTYPE Segment = :{start :Point, finish :Point}
LET a = (Point {x = 1, y = 2})
LET b = (Point {x = 3, y = 4})
LET seg = (Segment {start = a, finish = b})
PRINT seg.finish.x
```

```text
3
```

`.` must read off a name or a sub-expression result, not directly off a literal
— write `LET n = 5` then `n.x` rather than `(5).x`, which is a parse error.
Reading a field a record doesn't have is an error that names the type:

```koan
NEWTYPE Point = :{x :Number, y :Number}
LET p = (Point {x = 3, y = 4})
p.w
```

```text
error: shape error: `Point` has no field `w`
```

## Required fields and extra fields

A record type names the fields a value must have. Leaving one out is an error at
construction, as is giving a field the wrong type — and the message shows the
whole expected and actual record shape:

```koan
NEWTYPE Point = :{x :Number, y :Number}
Point {x = 3}
```

```text
error: type mismatch for argument 'value': expected :{x :Number y :Number}, got :{x :Number}
```

```koan
NEWTYPE Point = :{x :Number, y :Number}
Point {x = "oops", y = 4}
```

```text
error: type mismatch for argument 'value': expected :{x :Number y :Number}, got :{x :Str y :Number}
```

The required fields are a *minimum*, though — a record may carry **more** fields
than its type names, and the extras are kept and readable:

```koan
NEWTYPE Point = :{x :Number, y :Number}
LET p = (Point {x = 3, y = 4, z = 5})
PRINT p.z
```

```text
5
```

This is *width subtyping*: a wider record (more fields) stands in wherever a
narrower one is expected.

## Records and dispatch

A function parameter can be typed with a record schema `:{...}` directly, and it
matches any record that has at least those fields. A record literal also stands
on its own as an anonymous record value, which prints with its fields:

```koan
LET person = {name = "ada", age = 36}
PRINT person
```

```text
{name = ada, age = 36}
```

One limit to know: `.` reads fields only off a `NEWTYPE` record value. A bare
`{...}` record is for structural matching and projection, not field access — to
read its fields, accept it through a `NEWTYPE` record type.

Width subtyping has a cost in dispatch: a wide record can satisfy two different
field-subset schemas at once, with neither more specific, so a call is
ambiguous:

```koan
FN (PICK r :{x :Number, y :Str}) -> Str = ("got xy")
FN (PICK r :{x :Number, z :Str}) -> Str = ("got xz")
LET both = {x = 1, y = "a", z = "b"}
PICK both
```

```text
error: ambiguous dispatch: 2 candidates match PICK both with equal specificity
```

`(<fields>) FROM <record>` resolves this by *projecting* a record to exactly the
named fields, narrowing the type the dispatcher sees so just one overload
matches:

```koan
FN (PICK r :{x :Number, y :Str}) -> Str = ("got xy")
FN (PICK r :{x :Number, z :Str}) -> Str = ("got xz")
LET both = {x = 1, y = "a", z = "b"}
PRINT (PICK ((x y) FROM both))
```

```text
got xy
```

The projection narrows the *type*, not the stored value — the other fields are
still physically there, just invisible to dispatch through the projected view.
When you bind a projection, wrap the whole right-hand side: `LET v = ((x y) FROM
both)`.

Next: [Newtypes](08-newtypes.md).

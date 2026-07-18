# Values and types

Every value in Koan has a type, and every type has a name you can write in
source. This chapter covers the built-in values and the vocabulary for naming
types. You'll use these names whenever you annotate a function parameter, a
return type, or a field.

## Literals

The built-in scalar values are numbers, strings, booleans, and null:

```koan
PRINT 42
PRINT 3.14
PRINT "hello"
PRINT 'also a string'
PRINT true
PRINT null
```

```text
42
3.14
hello
also a string
true
null
```

Numbers are 64-bit floating point — there is no separate integer type, so `42`
and `42.0` are the same value and both print as `42`. Strings can use double or
single quotes interchangeably; `PRINT` renders a string without the quotes.

### Lists

A list is an ordered sequence written with square brackets. Commas between
elements are optional — whitespace alone separates them:

```koan
PRINT [1, 2, 3]
PRINT [1 2 3]
```

```text
[1, 2, 3]
[1, 2, 3]
```

Elements can themselves be expressions; they are evaluated before the list is
built.

### Dictionaries

A dictionary maps scalar keys to values, written with braces and `key: value`
pairs (commas again optional):

```koan
PRINT {"width": 4}
```

```text
{"width": 4}
```

Keys must be scalars — a number, string, boolean, or null. A *bare identifier*
used as a key is resolved as a name lookup, the same as anywhere else, so
`{user: 1}` looks up `user` and uses its value as the key. Use a quoted string
when you want a literal key. Dictionaries are unordered: don't rely on the
order keys come back in when you print one.

Braces are also used for *records* — `{field = value}` with an `=` instead of a
`:`. Records are how you construct [record types](07-records.md) and
[call functions by name](04-functions.md); they're a distinct form from
dictionaries, covered in those chapters.

### Empty collections need a type

An empty list or dictionary can't infer its element type on its own, so a bare
`[]` or `{}` is only valid where a type is already expected — for example as an
argument to a function whose parameter is typed. A standalone `[]`, or
`LET xs = []`, is an error. Once you've met [functions](04-functions.md) and
[type ascription](#naming-types) below, you can give an empty collection a type
through the position it appears in.

## Comparing values with `==` and `!=`

`==` tests whether two values are equal and `!=` whether they differ; both give
back a boolean:

```koan
PRINT (1 == 1)
PRINT (1 != 2)
PRINT ("a" == "b")
```

```text
true
true
false
```

Equality is **structural** — it looks at the contents, not at how a value was
built or rendered. Two lists are equal when they have the same elements in the
same order:

```koan
PRINT ([1, 2, 3] == [1, 2, 3])
PRINT ([1, 2] != [1, 2, 3])
```

```text
true
true
```

A few rules are worth knowing early:

- **Binary only.** `==` and `!=` compare exactly two values. There is no
  chaining: `a == b == c` is an error, not a three-way test.
- **Numbers follow IEEE.** In particular a not-a-number result is not equal to
  anything, including itself.
- **Records ignore field order.** `{x = 1, y = 2}` equals `{y = 2, x = 1}` (see
  [Records](07-records.md)).
- **Newtypes compare by identity.** A [newtype](08-newtypes.md) value is never
  equal to its bare representation, and two different newtypes with the same
  representation are unequal.
- **Functions and modules can't be compared** — the comparison is an error. To
  compare two modules by their interface, compare their types instead:
  `(TYPE OF m1) == (TYPE OF m2)` (see [Modules](11-modules.md)).

## Naming types

The type names you can write in source are:

| Type                          | What it is                          | Example value                  |
|-------------------------------|-------------------------------------|--------------------------------|
| `Number`                      | 64-bit float                        | `42`, `3.14`                   |
| `Str`                         | string                              | `"hi"`, `'hi'`                 |
| `Bool`                        | boolean                             | `true`, `false`                |
| `Null`                        | the null value                      | `null`                         |
| `:(LIST OF <element>)`        | ordered list                        | `[1, 2, 3]`                    |
| `:(MAP <key> -> <value>)`     | map / dictionary                    | `{"a": 1}`                     |
| `:(FN (<params>) -> <result>)`| function value                      | see [Functions](04-functions.md) |
| `Any`                         | wildcard — accepts any value        | used only in annotations       |

You'll also occasionally see `Type`, `Module`, `Signature`, and `KExpression`
in error messages or signatures — these are real types, but you rarely write
them by hand. `KExpression` is an unevaluated, [quoted](10-quoting.md)
expression carried as a value.

Types you declare yourself with [`UNION`](05-tagged-unions.md) and
[`NEWTYPE`](07-records.md) get their own names and join this vocabulary.

### Writing a type: the `:` sigil

You attach a type to something with the `:` sigil glued directly to the type,
with no space after the colon:

```koan
x :Number
```

For a parameterized type, the sigil opens a parenthesized group:

```koan
xs :(LIST OF Number)
ys :(MAP Str -> Number)
f  :(FN (n :Number) -> Str)
```

The spacing matters: write `x :Number` (space before the colon, glued after) or
`x:Number`, but **not** `x: Number` with a space after the colon — that is a
parse error. A non-parameterized type like `Number` may be written with the
sigil or as a bare token, but a parameterized `:(...)` type always needs it.

Container types are always parameterized down to their elements. The element
types of a list literal are joined: `[1, 2, 3]` is `:(LIST OF Number)`, while
`[1, "x"]` widens to `:(LIST OF Any)`.

### Types are values

A type expression is itself a value you can bind to a name. A type must bind to
a *type name* — capitalized with at least one lowercase letter (see
[token classes](03-names-and-dispatch.md#token-classes)):

```koan
LET Numbers = :(LIST OF Number)
PRINT Numbers
```

```text
:(LIST OF Number)
```

This is the foundation the [module system](11-modules.md) builds on, where
signatures describe types abstractly and modules supply them.

Next: [Names, binding, and dispatch](03-names-and-dispatch.md).

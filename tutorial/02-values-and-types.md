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

### Empty collections fit anywhere

An empty list has no elements to infer an element type from, so it takes `Never` —
the type no value ever has. That is what makes `[]` fit *any* list: `LET xs = []`
binds, a standalone `[]` is fine, and `[]` can be passed to a function whose
parameter is typed `:(LIST OF Number)`, because a list of nothing is a list of
numbers as far as the type is concerned. Once you've met
[functions](04-functions.md) and [type ascription](#ascribing-a-type-with-) below,
you can still pin an empty collection to a specific type:
`[] :! (LIST OF Number)` is an empty list of numbers. (Braces are different: a bare `{}` is the empty *record*, not an empty
dictionary.)

## Reading lists, dictionaries and strings

You read a list by applying it to an index, as you would call a function on
an argument. Indices start at zero, and a string is read the same way, one code
point at a time:

```koan
LET xs = [10 20 30]
PRINT (xs 1)
PRINT (xs 3)
PRINT ("héllo" 1)
```

```text
:(Ok {Error = Never, Ok = Number})(20)
:(Error {Error = Number, Ok = Never})(3)
:(Ok {Error = Never, Ok = Str})(é)
```

A read can miss, so it never hands back the element bare. It gives back a
`Result`, a built-in [tagged union](05-tagged-unions.md) with two variants:
`Ok` holding what was read, or `Error` holding the index that failed, which is
one out of range or not a whole number. `PRINT` shows the variant's type before
its payload; [`MATCH`](06-pattern-matching.md) is how you take a `Result` apart.

A dictionary is read by applying it to a key. A missing key is an `Error` of
that key, and a key of a type the dictionary can never hold is refused before
the program runs:

```koan
LET ages = {"ada": 36, "alan": 41}
PRINT (ages "ada")
PRINT (ages "grace")
```

```text
:(Ok {Error = Never, Ok = Number})(36)
:(Error {Error = Str, Ok = Never})(grace)
```

### Slices

Applied to a record naming `from`, `to` or both, a list or a string gives back
a **slice** of itself. A slice is half-open — it runs from `from` up to, but not
including, `to` — and a missing `from` starts at the beginning, a missing `to`
runs to the end. When `to` is below `from`, or is `-1`, the slice runs
backward: it starts at `from` and stops just past `to`, so `-1` reaches the
first element. Reversing a list or a string is a backward slice from its last
element:

```koan
LET xs = [10 20 30 40]
PRINT (xs {from = 1, to = 3})
PRINT (xs {from = 2})
PRINT (xs {from = 3, to = 0})
PRINT (xs {from = 3, to = -1})
PRINT ("koan" {from = 3, to = -1})
PRINT (xs {from = 0, to = 5})
```

```text
:(Ok {Error = Never, Ok = :(LIST OF Number)})([20, 30])
:(Ok {Error = Never, Ok = :(LIST OF Number)})([30, 40])
:(Ok {Error = Never, Ok = :(LIST OF Number)})([40, 30, 20])
:(Ok {Error = Never, Ok = :(LIST OF Number)})([40, 30, 20, 10])
:(Ok {Error = Never, Ok = Str})(naok)
:(Error {Error = Number, Ok = Never})(5)
```

A bound out of range gives back an `Error` of the first bad bound, `from`
before `to`. A slice has the type of the list it came from, so a slice of
`[1, "a"]` is still a `:(LIST OF (Number | Str))` even if it holds only the
`1`. Slicing copies nothing: a slice reads the elements where they already
are, however many slices of slices you take.

### Counting and spreading

`LENGTH` counts a list's elements, a dictionary's entries or a string's code
points. Inside a list literal, `..` glued to a list's name, or to a
parenthesized expression giving a list, lays that list's elements into the new
list in place:

```koan
LET xs = [1 2]
LET ys = [3]
PRINT (LENGTH xs)
PRINT (LENGTH {"a": 1, "b": 2})
PRINT (LENGTH "héllo")
PRINT [0 ..xs ..ys 4]
PRINT [..([]) ..xs]
```

```text
2
2
5
[0, 1, 2, 3, 4]
[1, 2]
```

Like a slice, a list built this way shares the elements it spreads rather than
copying them. `..` means nothing outside a list literal's elements: `LENGTH ..xs`
is a parse error.

## Building strings

A string written with `%` glued in front of its opening quote is a
**template**. Each `{…}` in it holds an expression, which is evaluated where the
template is and written into the text as `PRINT` would print it. A brace holds
one expression, exactly as a pair of parentheses does, and `{{` and `}}` write
a brace itself:

```koan
LET name = "Ada"
LET xs = [1 2]
PRINT %"Hello, {name}!"
PRINT %"{xs} has {LENGTH xs} elements"
PRINT %'{{braces}} and {"quotes"}'
```

```text
Hello, Ada!
[1, 2] has 2 elements
{braces} and quotes
```

A string inside a brace takes the other quote, since the template's own quote
would end it. `%{` always opens a brace of the second kind, below, so a `%`
just before a brace is written as a string of its own, `{'%'}{n}`.

A program writing text for another language — SQL, HTML, a shell command —
must treat the values it splices in apart from the text it trusts. A `%{…}`
brace **keeps** its value apart: a template holding one is a record of
`strings`, the pieces of text around the kept values, and `values`, the kept
values in order. A `{…}` brace still renders into the text beside it:

```koan
LET user = "x'; DROP TABLE users"
LET query = %"SELECT * FROM users WHERE name = %{user} AND age > %{30}"
PRINT query.strings
PRINT query.values
```

```text
[SELECT * FROM users WHERE name = ,  AND age > , ]
[x'; DROP TABLE users, 30]
```

There is always one more string than value, so a function taking such a record
can quote or escape each value for its language and interleave the two.

`JOIN` writes a list's elements one after another, each rendered as `PRINT`
renders it, and `JOIN … BY` puts a separator string between them:

```koan
PRINT (JOIN ["a" 1 [2]])
PRINT (JOIN ["red" "green" "blue"] BY ", ")
```

```text
a1[2]
red, green, blue
```

A string built by a template or by `JOIN` shares the strings it is made from
rather than copying them, as a slice does.

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
- **Functions and modules can't be compared** — the comparison is an error.

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
| `:(FN :{<params>} -> <result>)`| function value                     | see [Functions](04-functions.md) |
| `Value`                       | any ordinary value                  | `42`, `"hi"`, `[1, 2]`         |
| `Type`                        | any type                            | `Number`, `:(LIST OF Str)`     |
| `Code`                        | any [quoted](10-quoting.md) code: a `Block`, an `Expression`, a `Name`, … | `#(PRINT "hi")` |
| `Any`                         | wildcard — accepts any value, type or code | used only in annotations |

`Value`, `Type` and `Code` split everything `Any` accepts into three families
that never overlap: a slot typed `Value` never receives a type or a piece of
code. You'll also occasionally see `Module` and `Signature` in error messages
or signatures — these are real types, but you rarely write them by hand. The
kinds of code under `Code` — `Block`, `Expression`, `Name` and the rest — say
what a [quoted](10-quoting.md#what-kind-of-code-a-quote-is) piece of code is.

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
f  :(FN :{n :Number} -> Str)
```

A function type's parameter list is a [record type](07-records.md) — the same
`:{name :Type}` form you write anywhere else — so a no-argument function type is
`:(FN :{} -> Str)`.

The spacing matters: write `x :Number` (space before the colon, glued after) or
`x:Number`, but **not** `x: Number` with a space after the colon — that is a
parse error. A non-parameterized type like `Number` may be written with the
sigil or as a bare token, but a parameterized `:(...)` type always needs it.

Container types are always parameterized down to their elements. A list's
element type is the union of its elements' types: `[1, 2, 3]` is
`:(LIST OF Number)`, and `[1, "x"]` is `:(LIST OF (Number | Str))`. A list of
records keeps every field each record was written with
([records in a container](07-records.md#records-in-a-container)).

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

A lowercase name can hold a type too, as data: you can pass it to a function,
print it, or compare it. But only a type name stands where a type goes, so
`LET Alias = t` is rejected even when `t` holds a type.

This is the foundation the [module system](11-modules.md) builds on, where
signatures describe types abstractly and modules supply them.

## Ascribing a type with `:!`

A value's type comes from its contents: `[1, 2]` is a `:(LIST OF Number)`.
`<value> :! <Type>` checks a value against a type and gives back the same value
viewed at that type, so whatever uses it next sees the type you wrote rather
than the contents' extra precision. [Overloading](04-functions.md#overloading-by-specificity)
shows the difference:

```koan
EXPR #(DESCRIBE xs :(LIST OF Number)) -> Str = #("numbers")
EXPR #(DESCRIBE xs :(LIST OF Any)) -> Str = #("anything")
PRINT (DESCRIBE [1, 2])
PRINT (DESCRIBE ([1, 2] :! (LIST OF Any)))
```

```text
numbers
anything
```

The type on the right is written as in any type position: a type name, a
parenthesized type like `(LIST OF Any)`, or the `:(…)` form. Ascribing a union
views the value at the member that fits it: after
`LET Maybe = :((LIST OF Any) | Null)`, `[1, 2] :! Maybe` is a
`:(LIST OF Any)`. A type with no list member — `Any` itself — leaves a list's
type as it was.

A value that does not satisfy the type is an error. Where koan can tell before
running that it never could, it refuses the program when it loads:

```koan
PRINT ("s" :! Number)
```

```text
error: <input>:1:7: this value is Str, which can never satisfy its ascription Number
```

`:!` over a [module](11-modules.md#ascription) checks the module against a
signature instead.

Next: [Names, binding, and dispatch](03-names-and-dispatch.md).

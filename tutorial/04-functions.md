# Functions

A function adds a new *shape* to the language: a pattern of keywords and typed
slots that, once defined, any later expression can match. Defining functions is
the main way you extend Koan.

## Defining and calling

`FN (<signature>) -> <ReturnType> = (<body>)` registers a function. The
signature is a parenthesized mix of fixed keywords and typed parameter slots;
the body is a parenthesized expression evaluated each time the function is
called.

```koan
FN (ECHO x :Number) -> Number = (x)
PRINT (ECHO 21)
```

```text
21
```

You call a function by writing its shape with values in the slots — here,
the `ECHO` keyword followed by a number. A parameter slot is always
`name :Type`, with the `:` glued to the type (see
[the `:` sigil](02-values-and-types.md#writing-a-type-the--sigil)). Both the
parameter types and the return type are required; a bare `x` with no `:Type` is
an error.

### Keywords can sit anywhere

The keyword doesn't have to come first. Putting it between two slots gives an
infix shape:

```koan
FN (a :Str OR b :Str) -> Str = (a)
PRINT ("first" OR "second")
```

```text
first
```

Functions can take several parameters, and commas between slots are optional:

```koan
FN (BETWEEN a :Number AND b :Number) -> Number = (a)
PRINT (BETWEEN 3 AND 9)
```

```text
3
```

Remember that keywords are fixed words with two or more capitals and no
lowercase (`ECHO`, `OR`, `BETWEEN`, `AND`), while parameter names are lowercase
identifiers. A signature must contain at least one keyword — there has to be a
fixed word for the shape to dispatch on:

```koan
FN (x :Number) -> Number = (x)
```

```text
error: shape error: FN signature must contain at least one Keyword (a fixed token to dispatch on)
```

## Return types are enforced

The declared return type is checked against the body's value every time the
function runs. A mismatch is an error:

```koan
FN (WRONG x :Number) -> Str = (x)
WRONG 5
```

```text
error: type mismatch for argument '<return>': expected Str, got Number
  in fn(WRONG <x>) (fn(WRONG <x>))
```

The indented `in …` line is the call trace that every error carries;
[Errors](09-errors.md) covers how to read and catch them.

This has one consequence worth internalizing early: **`PRINT` evaluates to the
string it printed**, not to null. So a function whose body is a `PRINT` returns
a `Str`:

```koan
FN (ANNOUNCE msg :Str) -> Str = (PRINT msg)
ANNOUNCE "starting up"
```

```text
starting up
```

If you annotated `ANNOUNCE` as `-> Null` it would fail the return check. A
function that genuinely produces nothing returns the `null` literal and is
annotated `-> Null`. Use `-> Any` to opt out of return checking entirely.

## Overloading by specificity

Because dispatch matches on slot *type*, several functions can share a keyword
as long as their slots differ. The most specific match wins, and a more precise
container type beats a looser one:

```koan
FN (SIZE xs :(LIST OF Number)) -> Str = ("numbers")
FN (SIZE xs :Any) -> Str = ("something else")
PRINT (SIZE [1, 2, 3])
PRINT (SIZE "hi")
```

```text
numbers
something else
```

`:(LIST OF Number)` is more specific than `:Any`, so the list routes to the
first definition and everything else falls through to the second.

## Functions as values

`FN` returns the function it registered, so you can capture it with `LET` and
pass it around. A captured function is called with **named arguments**: one
record literal `{name = value}`, with each argument introduced by its parameter
name. Argument order is independent of the declaration:

```koan
LET pick =
  FN (a :Str OR b :Str) -> Str = (a)
PRINT (pick {a = "first", b = "second"})
PRINT (pick {b = "second", a = "first"})
```

```text
first
first
```

Leaving out a required name is an error:

```koan
LET pick =
  FN (a :Str OR b :Str) -> Str = (a)
pick {a = "only"}
```

```text
error: missing argument 'b'
```

### Anonymous functions

A function whose signature is just a record schema — `FN :{<fields>} -> Type`
— has no keyword, so it registers no shape. The value `FN` returns is the only
way to call it, always by named record:

```koan
LET label = (FN :{text :Str} -> Str = (text))
PRINT (label {text = "hi"})
```

```text
hi
```

### Closures

A function body can refer to names from where the function was *defined*,
including a parameter of an enclosing function. The inner function carries those
captures with it:

```koan
FN (CONSTANTLY value :Str) -> :(FN () -> Str) =
  FN :{} -> Str = (value)
LET always_hi = (CONSTANTLY "hi")
PRINT (always_hi {})
```

```text
hi
```

`CONSTANTLY` returns a fresh zero-argument function that closes over `value`. Its
return type, `:(FN () -> Str)`, is the type of that function — a function that
returns a function declares the function type it produces, and the returned
function is checked against it.

## There are no loops

Koan has no loop constructs, and no arithmetic or comparison operators either.
Iteration is expressed with **recursion**: a function that calls itself, with a
base case selected by dispatch. The natural way to write the base case is to
match on a [tagged union](05-tagged-unions.md), so the full recursion idiom
comes together in [Pattern matching](06-pattern-matching.md).

Next: [Tagged unions](05-tagged-unions.md).

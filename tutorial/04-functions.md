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
the `ECHO` keyword followed by a number. Every slot is filled with a *value*:
each argument evaluates before the call it belongs to, so a shape you define
cannot leave one of its arguments unrun. A shape that wants code takes it as a
quoted value — see [Quoting and evaluating](10-quoting.md#passing-code-to-a-function-you-wrote). A parameter slot is always
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
  in :(FN :{x :Number} -> Str) (WRONG 5) at <input>:2:1
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

There are three function forms, and which one you write is the choice of how the
function can be reached:

- **`FN (<signature>) -> <Type> = (<body>)`** — the bare named form above. It
  registers a shape, so it is reached by writing that shape (`ECHO 21`). It binds
  no name.
- **`FN :{<fields>} -> <Type> = (<body>)`** — the anonymous form. No keyword, so
  no shape is registered; the function is only the value the expression produces,
  and you bind that value with `LET`.
- **`LET <name> = FN (<signature>) -> <Type> = (<body>)`** — the combined form.
  One statement, one definition, reached *both* ways: the shape dispatches and
  the name holds the same function.

`LET <name> = OP …` (and the `UNARY OP` twin) does the same for operators.

Writing `LET <name> = FN …` as one statement binds a name to the function
*and* registers its shape — one declaration reaching both. To break it across
lines, end the line with `,`: a bare indented continuation is read as a nested
expression, which puts the definition back in a value slot. A function bound
this way is called with **named arguments**: one record literal
`{name = value}`, with each argument introduced by its parameter name. Argument
order is independent of the declaration:

```koan
LET pick = ,
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
LET pick = ,
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
FN (CONSTANTLY value :Str) -> :(FN :{} -> Str) =
  FN :{} -> Str = (value)
LET always_hi = (CONSTANTLY "hi")
PRINT (always_hi {})
```

```text
hi
```

`CONSTANTLY` returns a fresh zero-argument function that closes over `value`. Its
return type, `:(FN :{} -> Str)`, is the type of that function — a function that
returns a function declares the function type it produces, and the returned
function is checked against it.

### Severing captures with `CLOSE OVER`

A closure holds on to the call it was defined inside — that is what makes
`value` readable after `CONSTANTLY` has returned. When the returned function
outlives that call by a long way, holding the whole call is more than you
asked for. `CLOSE OVER` runs a block over a workspace of its own and *copies*
the values you name into it, so what escapes carries copies rather than a
handle on the call that built it:

```koan
FN (GREETER text :Str) -> :(FN :{} -> Str) =
  CLOSE OVER (text) (
    FN :{} -> Str = (text)
  )
LET hi = (GREETER "hi")
PRINT (hi {})
```

```text
hi
```

The parentheses after `CLOSE OVER` are the **capture list**: the names copied
in, one per name, and `CLOSE OVER ()` names none. Inside the block you can see
the captures, anything the block itself binds, and every top-level and
built-in definition — but *not* the rest of the enclosing call, so a value
from the enclosing function that you did not capture is unbound there.
Definitions are the exception: shapes defined with `FN`, operators declared
with `OP`, and modules all come along on their own, so the block can still
call them.

A shape is not a value, so a capture list names one by its call pattern with
`_` in each slot: `CLOSE OVER ((HELPER _)) (...)` names the `HELPER x`
function, spelling out what the block leans on. Only the block's last
expression escapes; anything it binds along the way stays inside.

### Letting koan work out the capture list

Writing the list out is often re-typing what the block already says. `CLOSE`
with no list infers one: koan reads the block and captures exactly the names it
uses from the enclosing call.

```koan
FN (GREETER text :Str) -> :(FN :{} -> Str) =
  CLOSE (
    FN :{} -> Str = (text)
  )
LET hi = (GREETER "hi")
PRINT (hi {})
```

```text
hi
```

The rules are the ones you would apply by hand. A name the block binds itself is
not captured, and neither is a nested function's parameter. A name that lives at
the top level or in the built-ins is read where it lives rather than copied. A
name that is bound nowhere at all is an error at the `CLOSE`, just as naming it
in a capture list would be. `CLOSE OVER ()` is still the way to say "capture
nothing" — an empty list is a list, not an omitted one.

Two forms are refused inside an inferred block:

```koan
MODULE m = (LET v = 1)
LET x = (CLOSE (USING m SCOPE (v)))
```

```text
error: CLOSE: `USING ... SCOPE` at <input>:2:16 surfaces module members dynamically, so the block's capture list cannot be inferred — name the captures with `CLOSE OVER (<names>) (<block>)`
  in LET x = <staged> (<bind>) at <input>:2:1
```

`$(...)` (see [Quoting](10-quoting.md)) and `USING … SCOPE` (see
[Modules](11-modules.md)) both work out which names they mean while the program
runs, so reading the block's text cannot tell which names the enclosing call has
to supply. Write the captures out with `CLOSE OVER` when you need either.

## There are no loops

Koan has no loop constructs, and no arithmetic or comparison operators either.
Iteration is expressed with **recursion**: a function that calls itself, with a
base case selected by dispatch. The natural way to write the base case is to
match on a [tagged union](05-tagged-unions.md), so the full recursion idiom
comes together in [Pattern matching](06-pattern-matching.md).

Next: [Tagged unions](05-tagged-unions.md).

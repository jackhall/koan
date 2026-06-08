# Koan Tutorial

A tour of the Koan language as it stands today. The focus is on how source is
structured and what the language gives you to compose with; each builtin gets a
short reference at the end. For the runtime architecture (parse → dispatch →
execute) see [README.md](README.md).

## Running a program

The CLI in [src/main.rs](src/main.rs) takes source from a file argument or from
stdin:

```sh
cargo run -- hello.koan
echo 'PRINT "hello"' | cargo run
```

A program is a sequence of top-level expressions, one per (logical) line. Each
runs in submission order against the same scope; failures surface as structured
[`KError`](src/machine/core/kerror.rs) values printed to stderr.

## Source structure

A Koan source file is a list of top-level expressions. An expression is an
ordered sequence of *parts*: tokens, literals, and nested sub-expressions. The
parser produces one [`KExpression`](src/machine/model/ast.rs) per top-level line
and hands it to dispatch.

### Tokens

Every non-literal atom falls into one of three classes, decided by the casing
rule in [tokens.rs](src/parse/tokens.rs):

| Class        | Rule                                                                      | Examples                        |
|--------------|---------------------------------------------------------------------------|---------------------------------|
| `Keyword`    | pure-symbol, **or** alphabetic with ≥2 uppercase letters and no lowercase | `=`, `->`, `:\|`, `LET`, `THEN` |
| `Type`       | first char uppercase **and** at least one lowercase elsewhere             | `Number`, `KFunction`, `IntOrd` |
| `Identifier` | lowercase- or `_`-leading                                                 | `x`, `greeting`, `my_var`       |

A token that starts uppercase but classifies as neither (e.g. a single
uppercase letter `A`, or `K9`) is a parse error rather than falling through to
identifier — the rule reserves uppercase-leading shapes as type-position
territory.

The split is load-bearing: only `Keyword` parts contribute fixed tokens to a
function signature's bucket key. Identifiers, types, literals, and nested
expressions are *slots* — they compete on type specificity, not on text.

### Literals

```
42            # number (f64)
3.14          # number
"hello"       # string
'hello'       # string (single quotes equivalent)
true   false  # booleans
null          # the null value
[1, 2, 3]     # list literal (commas optional: [1 2 3] works too)
{a: 1, b: 2}  # dict literal (commas optional: {a: 1 b: 2} works too)
```

A bare literal on its own is a complete top-level expression — it dispatches
through `value_pass` and produces the value, but with nothing consuming the
result it's effectively a no-op.

List elements and dict values can be sub-expressions; the scheduler resolves
them before the parent dispatches. Dict keys must be scalar (number / string /
bool / null); a bare-identifier key resolves via name lookup, like Python.

### Grouping: parens and indentation

Parentheses group a sequence of parts into a *nested expression* that
dispatches on its own; its result is spliced back into the enclosing
expression as a `Future`. So in

```
PRINT (LET msg = "hello world!")
```

the inner `LET` runs first, returns the bound value, and that value becomes
`PRINT`'s `msg` argument.

Two-space indentation under a parent line is desugared to parens by the
[whitespace pass](src/parse/whitespace.rs) before any further parsing happens.
These two programs are identical:

```
PRINT (LET msg = "hi")
```

```
PRINT
  LET msg = "hi"
```

Rules:

- Tabs are rejected.
- Indents must be in multiples of two spaces.
- Blank lines are ignored.

Evaluation order: a nested `(...)` whose result feeds a *value* slot is
evaluated **eagerly**, before its parent dispatches — the parent sees the
computed value. A `(...)` that feeds an *expression* slot is **lazy** — it
rides through to the parent as data, and the parent decides whether (and
when) to dispatch it. `MATCH ... WITH`, the body of `FN`, and the schemas of
`UNION` are the cases where laziness shows up today.

### Compound-token operators

A small registry in [operators.rs](src/parse/operators.rs) desugars
character-trigger operators into keyword-headed sub-expressions at parse time:

| Surface | Desugars to               | Kind   |
|---------|---------------------------|--------|
| `!x`    | `(NOT x)`                 | prefix |
| `a.b`   | `(ATTR a b)`              | infix  |
| `x?`    | `(TRY x)`                 | suffix |

These are pure syntax — there are no runtime builtins for `NOT`, `ATTR`, or
`TRY` yet, so the desugared forms parse but won't dispatch. They exist as
plumbing for future work.

## Names and dispatch

`LET <name> = <value>` introduces a name. Once introduced, the name is
visible to every expression that follows it in the same block, and to any
nested block. A bare identifier in any value-typed slot is a name lookup —
both the bare-identifier-on-its-own form and identifiers passed as
arguments to a builtin or user function:

```
LET msg = "hi"
PRINT msg          # prints "hi" — `msg` in PRINT's value slot resolves to "hi"
LET copy = msg     # binds `copy` to "hi", not to the literal string "msg"
```

Name resolution is **lexical**: a reference sees a binding only if the
binding lexically precedes it. A forward value reference between sibling
top-level expressions is `UnboundName`:

```
LET y = x
LET x = 42
PRINT y            # UnboundName: x
```

Reorder to put the binder first and the reference resolves:

```
LET x = 42
LET y = x
PRINT y            # prints 42
```

Nominal binders — `STRUCT`, named `UNION`, `SIG`, `FUNCTOR`, `MODULE` —
carve themselves out of the lexical rule: their declared names are visible
to siblings on both sides, so mutual references between sibling type
declarations work without reordering. Function bodies re-dispatch each time
they're called against the body's own lexical chain, so mutual recursion
between sibling FNs works (each call resolves the other from inside an
already-running body, not from the outer block's cutoff).

A name that no binder visibly introduces surfaces as `UnboundName`.
Re-binding a value name in the same scope surfaces as `Rebind`; shadowing
across nested scopes (a child block, a function body) is allowed and is
how lexical scoping works.

A *visible* binding whose producer hasn't finished computing — typically a
binding whose right-hand side is still running — *parks* the consumer on
the producer rather than failing; the consumer wakes and resumes once the
producer's value lands.

When you write an expression, Koan picks which function (builtin or
user-defined) to run by matching the *shape* of what you wrote — the
keywords, the slot count, and the slot types — against the shapes of every
function in scope. Two functions can share keywords as long as their slot
types differ; the more specific match wins. The keyword `PRINT` in a
single-slot context, for example, is always the `PRINT` builtin because
nothing else has that shape.

User-defined functions follow the same model: `FN` registers a new shape,
and any later expression that matches it is routed to that function. Names
introduced with `LET` shadow names in outer blocks for the rest of the
current block.

## Types

Every value in Koan has a type. The names you can write in source are:

| Type                       | What it is                          | How to write a value                      |
|----------------------------|-------------------------------------|-------------------------------------------|
| `Number`                   | 64-bit float                        | `42`, `3.14`                              |
| `Str`                      | string                              | `"hi"`, `'hi'`                            |
| `Bool`                     | boolean                             | `true`, `false`                           |
| `Null`                     | the null value                      | `null`                                    |
| `:(LIST OF T)`             | ordered sequence                    | `[1, 2, 3]`                               |
| `:(MAP K -> V)`            | scalar-keyed map                    | `{a: 1, b: 2}`                            |
| `:(FN (args) -> R)`        | callable function value             | `(FN (DOUBLE x :Number) -> Number = (x))` |
| `Tagged`                   | a value of a tagged union           | `Maybe (Some 42)` (see `UNION` below)     |
| `Any`                      | wildcard — accepts any value        | (used in annotations only)                |

A type name appears wherever you annotate something: the type of a parameter
slot (`x :(LIST OF Number)`), the return type on a function (`-> Number`), the
payload type of a tagged-union variant (`Some :Number`). Ascriptions use the
glued-right `:` sigil with no space between the `:` and the type — `x :Number`,
not `x: Number`. Parameterized type expressions extend the same form into an
S-expression group: `:(LIST OF Number)`, `:(MAP Str -> Number)`,
`:(FN (x :Number) -> Str)`. Bare non-parameterized type tokens in
non-ascription positions (e.g. the RHS of `LET Type = Number`) keep working
without the sigil.

Container types are always parameterized — bare `List` lowers to
`:(LIST OF Any)`, bare `Dict` to `:(MAP Any -> Any)`. There is no bare `Function`;
write `:(FN (args) -> R)` for a typed function or `Any` for an
unconstrained value (a function with no signature has nothing to dispatch
on).

You'll also see `KExpression` (an unevaluated parenthesized expression carried
as data) referenced in builtin signatures and error messages — it's a real
type, but you rarely write it yourself. List/dict literal types are inferred
as the join of element types: `[1, 2, 3]` is `:(LIST OF Number)`, `[1, "x"]` is
`:(LIST OF Any)`, `[]` is `:(LIST OF Any)`.

## User-defined functions

`FN <signature> -> <ReturnType> = <body>` registers a function. The signature
is a parens-wrapped expression mixing fixed `Keyword` tokens (the dispatch
shape) and typed parameter slots written as `name :Type` (glued-right `:` for
parameterized types; the space form `name: Type` is accepted for bare types).
The body is a parens-wrapped expression evaluated at call time.

```
FN (DOUBLE x :Number) -> Number = (x)
FN (a :Str SAID) -> Null = (PRINT a)            # infix-shaped — keyword in non-leading position
FN (FIRST x :Str y :Str) -> Null = (PRINT x)    # multiple params
FN (ADD x :Number, y :Number) -> Number = (x)   # commas optional, same shape
FN (HEAD xs :(LIST OF Number)) -> Number = (1)  # parameterized container in a slot
FN (NUMS) -> :(LIST OF Number) = ([1 2 3])      # parameterized return type

DOUBLE 21        # → 21
"hi" SAID        # prints "hi"
FIRST "a" "b"    # prints "a"
```

Both the parameter types and the return type are **non-optional**. A bare
`x` without `:Type` is a parse error. Calls whose argument types don't satisfy
the signature fail at dispatch (`KErrorKind::DispatchFailed`); the same call
shape with different parameter types routes to a different overload by
slot-specificity (more specific wins — `:(LIST OF Number)` beats `:(LIST OF Any)` beats
`Any`). Use `:Any` to opt a slot out of type checking.

The return type is **enforced at runtime**. A body whose result doesn't match
the declared type fails with `KErrorKind::TypeMismatch { arg: "<return>", … }`.
For parameterized container returns, the check walks elements: a function
declared `-> :(LIST OF Number)` whose body returns `[1, "x"]` errors with
`expected :(LIST OF Number), got :(LIST OF Any)`. Use `-> Any` to opt out.

A signature must contain at least one `Keyword` (the dispatch token); otherwise
it would shadow `value_lookup`/`value_pass`.

`FN` returns the registered function value, so you can capture it as a value:

```
LET f = (FN (DOUBLE x :Number) -> Number = (x))
f {x = 21}        # → 21, via call_by_name (named arguments)
```

Function calls through `call_by_name` use **named arguments**: each value is
introduced by its parameter name and `=`, inside one record literal `{...}`.
Order is independent of the declaration:

```
LET pair = (FN (a :Number TIMES b :Number) -> Number = (a))
pair {a = 3, b = 4}        # → 3
pair {b = 4, a = 3}        # → 3 (same call, different argument order)
```

Missing names error with `KErrorKind::MissingArg`; unknown names with
`KErrorKind::ShapeError`.

Free names in a body resolve through the FN's *captured* definition scope —
true lexical scoping, including for closures returned from another function's
body. Recursion is the iteration model; tail calls reuse the calling slot.

## Tagged unions

`UNION` declares a type whose values carry a *tag* and a payload:

```
UNION Maybe = (Some :Number None :Null)
```

A tag is a **capitalized** type-name token (`Some`, not `some`); the payload is a
type-name token too. The schema body is a parens-wrapped sequence of `<Tag>
:<Type>` pairs. A lowercase tag is a parse error. Every UNION carries a
per-declaration identity — the bare `UNION (...)` form is not accepted.

Construct a value by calling the type with a `(Tag value)` pair:

```
LET m = (Maybe (Some 42))
```

A type aliases only under a Type-classified (uppercase-leading) name — e.g.
`LET Maybe2 = Maybe`, then `Maybe2 (...)` constructs through the same lane. A
type can never bind to a value-classified (lowercase) identifier: `LET maybe =
Maybe` is rejected.

Each variant is its own type, reached through its union with the
union-qualified sigil `:(Maybe Some)`. A slot typed `:(Maybe Some)` admits only
`Some` values, while a `:Maybe` slot admits any variant — so functions can
dispatch on a single variant:

```
FN (DESC x :(Maybe Some)) -> :Str = ("is-some")
FN (DESC x :(Maybe None)) -> :Str = ("is-none")
PRINT (DESC (Maybe (Some 1)))     ; is-some
```

Pattern-match on the tag with `MATCH ... WITH`. The branches are
`<Tag> -> <body>` triples. A trailing comma joins the next line into the
same group:

```
MATCH (m) WITH
  Some -> (PRINT "got"),
  None -> (PRINT "no")
```

Only the matching branch's body is dispatched. Inside a branch, `it` is bound
to the inner value:

```
UNION Outcome = (Ok :Str Err :Str)
LET r = (Outcome (Ok "all good"))
MATCH (r) WITH (Ok -> (PRINT it) Err -> (PRINT "failed"))
```

A non-exhaustive match (no branch for the actual tag) errors with
`KErrorKind::ShapeError`.

## Structs

`STRUCT` declares a record type — an ordered list of named fields, each with
a declared type. The form mirrors `UNION`:

```
STRUCT Point = (x :Number, y :Number)
STRUCT User = (id :Number, name :Str, active :Bool)
```

Construction is **named**: each value is introduced by its field name and `=`,
inside one record literal `{...}`. Order is independent of the declaration — the
constructor reorders the fields into schema order before validating types:

```
LET p = (Point {x = 3, y = 4})
LET u = (User {id = 42, name = "alice", active = true})
LET q = (Point {y = 4, x = 3})             # same struct as p
```

Bare identifiers on the value side resolve through scope just like literals do —
no extra parens needed:

```
LET vx = 7
LET vy = 11
LET q = (Point {x = vx, y = vy})
```

Missing or unknown field names, and wrong field-type values, all error at
construction time:

```
LET bad = (Point {x = "oops", y = 4})
# error: type mismatch for argument 'x': expected Number, got Str

LET partial = (Point {x = 3})
# error: missing argument 'y'
```

A struct value's runtime type is a `KType::SetRef` into the type's sealed
`RecursiveSet` (a singleton set for a non-recursive struct) — the
per-declaration identity, so values of distinct STRUCT declarations dispatch
to different overloads. The schema itself (the `StructType` carrier `Point`
is bound to) is `KType::Type`, shared with `TaggedUnionType`.

Read a field off a struct value with the `.` operator (an alias for the
`ATTR` builtin):

```
LET dx = p.x                             # 3
PRINT (p.y)                              # 4

STRUCT Line = (start :Struct, finish :Struct)
LET seg = (Line {start = p, finish = q})
LET tipx = seg.finish.x                  # chained: 3
```

Reading a missing field, or applying `.` to a non-struct value, errors at
access time:

```
LET bogus = p.z
# error: shape error: struct `Point` has no field `z`

LET wat = (5).x
# error: type mismatch for argument 's': expected Struct, got Number
```

## Newtypes

`NEWTYPE` declares a fresh nominal type identity over a transparent
representation. Unlike `STRUCT`, which mints a new identity *and* a new
field-based shape, `NEWTYPE Distance = Number` says "a `Distance` is
represented as a `Number`, but the two are distinct types at dispatch":

```
NEWTYPE Distance = Number
NEWTYPE Duration = Number
```

Construct a value by calling the type with its representation:

```
LET d = (Distance 3.0)
LET t = (Duration 3.0)
```

Even though both carry the same `Number` underneath, they're observably
distinct at dispatch — a slot typed `Number` rejects a `Distance`, and a
slot typed `Distance` rejects a raw `Number`:

```
FN (KM_FROM x :Number)   -> Str = ("got Number")
FN (KM_FROM x :Distance) -> Str = ("got Distance")

KM_FROM 3.0        # → "got Number"   (raw Number; Distance slot rejects)
KM_FROM d          # → "got Distance" (Distance value; Number slot rejects)
```

This is what `STRUCT` can't give you ergonomically: pairs like
(`UserId`, `PostId`) or (`Distance`, `Duration`) that share a representation
but mean different things, without wrapping each in a single-field record.

The representation can be any type, including another struct:

```
STRUCT Point = (x :Number, y :Number)
NEWTYPE Boxed = Point

LET p = (Point {x = 1, y = 2})
LET b = (Boxed p)
```

Field access *falls through* the wrapper for struct representations, so
you can read fields off a `Boxed` value as if it were a `Point`:

```
LET bx = b.x        # 1
LET by = b.y        # 2
```

Missing-field errors name the inner struct, not the newtype — the
fall-through is transparent at the diagnostic level too:

```
LET bogus = b.z
# error: shape error: struct `Point` has no field `z`
```

Newtype-over-newtype is collapsed to a single layer at construction time —
no matter how many levels of NEWTYPE wrap a value, the inner representation
sits exactly one wrapper deep. `Distance("hi")` (a non-`Number` value for a
`Number`-repr NEWTYPE) fails at construction with `KErrorKind::TypeMismatch`.

The wildcard slot type for "any newtype" is not yet a writable surface name
— it's reserved for when a builtin signature surfaces the need.

## Quoting and evaluating expressions

Two prefix sigils give you surface control over when an expression evaluates.
`#(expr)` *quotes*: the parenthesized body is captured as a `KExpression`
value with no evaluation. `$(expr)` *evals*: the operand is resolved to a
value, and if that value is a `KExpression` the captured AST is dispatched
in the surrounding scope.

```
LET q = #(PRINT "hi")     # q is a KExpression value; PRINT does not run
$(q)                      # runs the captured AST — prints "hi"
```

Together they let user code thread raw ASTs through eager-evaluating
positions (dict values, list elements, function arguments) and thread
`KExpression` values back through the lazy slots that would otherwise consume
raw AST. EVAL returns whatever the inner AST evaluates to; an EVAL of any
non-`KExpression` value errors with `KErrorKind::TypeMismatch`.

The surface is paren-only: the sigil and its `(` must be adjacent. `#foo`,
`# (foo)`, `#42`, and `#}` all parse-error with `expected '(' after '#',
found <c>`. The indent-driven block syntax has one exception — a sigil-led
continuation line collapses through the wrap rule, so

```
LET q =
  #(1)
```

is equivalent to `LET q = #(1)`. Comma-continuation and bracket/dict
continuation lines do not get this rewrite — a bare `#sym` reaches the
parser unchanged and fails the sigil-adjacency rule, so spell out the parens
explicitly when continuing into a list, dict, or trailing-comma chain.

## Errors

Failures are first-class [`KError`](src/machine/core/kerror.rs) values with a
`kind` and a chain of frames showing where it came from. The CLI prints them
to stderr:

```
$ echo 'foo' | cargo run
error: unbound name 'foo'

$ printf 'FN (LIE) -> Number = ("oops")\nLIE\n' | cargo run
error: type mismatch for argument '<return>': expected Number, got Str
  in fn(LIE) (fn(LIE))
```

Variants you can hit today: `TypeMismatch`, `MissingArg`, `UnboundName`,
`ArityMismatch`, `AmbiguousDispatch`, `DispatchFailed`, `ShapeError`,
`ParseError`, `Rebind` (a second `LET <name>` against a name already bound
in the same scope), `DuplicateOverload` (an `FN` whose signature exactly
matches an already-registered overload), `User`. Uncaught errors
short-circuit to the top level. Intentional `null` values (the `null`
literal, `PRINT`'s return) are not errors.

### Catching errors with `TRY ... WITH`

`TRY (<expr>) WITH (<branches>)` evaluates `<expr>` in a catching context
and dispatches a branch keyed on the result. Each branch is a
`<Tag> -> <body>` triple: `Ok` runs on success with `it` bound to the
value, the capitalized `KErrorKind` names (`TypeMismatch`, `MissingArg`,
`UnboundName`, `ArityMismatch`, `AmbiguousDispatch`, `DispatchFailed`,
`ShapeError`, `ParseError`, `User`) catch the matching error with `it`
bound to a per-variant payload struct, and `_` is an optional wildcard:

```
TRY (RISKY x) WITH
  Ok           -> (PRINT it)
  TypeMismatch -> (PRINT it.expected)
  _            -> (PRINT "something else went wrong")
```

Each error arm's `it` payload carries the variant's structured fields
plus `it.frames :List<Str>` (one entry per call frame, rendered
`"in <expression> (<function>)"`). The `Ok` arm binds `it` to the bare
success value, not a wrapper. No matching arm and no `_` re-raises the
original error; a successful expression with no `Ok` arm raises a
synthetic `ShapeError`. The TRY body and each WITH arm are their own
lexical blocks — a `LET` inside the body or an arm is local to that
arm and is not visible after the `TRY`. See
[design/error-handling.md](design/error-handling.md) for the full per-arm
shape table.

## Putting it together

```
UNION Greeting = (Formal :Str Casual :Str)

FN (SAY msg :Str) -> Null = (PRINT msg)

LET hello = (Greeting (Casual "hey"))

MATCH (hello) WITH
  Formal -> (SAY "greetings, sir"),
  Casual -> (SAY it)
```

What runs:

1. `UNION Greeting = ...` registers a tagged-union type with two variants.
2. `FN (SAY msg :Str) -> Null = (PRINT msg)` defines a one-arg function over
   strings.
3. `LET hello = (Greeting (Casual "hey"))` builds a `Tagged` value with tag
   `Casual` and payload `"hey"`, and binds it as `hello`.
4. `MATCH` sees the `Casual` tag, runs the `Casual` branch, and `SAY it`
   prints `hey`.

## Builtin reference

One line per surface form. Sources under
[src/builtins/](src/builtins).

| Form                                     | Effect                                                                                          | File                                                          |
|-------------------------------------------------------|-------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| `LET <name> = <value>`                   | Bind `<name>` to `<value>` in the current scope. Returns the bound value.                       | [let_binding.rs](src/builtins/let_binding.rs)        |
| `PRINT <msg:Str>`                        | Write `<msg>` and a newline to the scope's output sink. Returns null.                           | [print.rs](src/builtins/print.rs)                    |
| `FN <sig> -> <Type> = <body>`            | Register a user function. Parameter slots in `<sig>` are typed (`name: Type`); the return type is runtime-enforced. Returns the function. | [fn_def.rs](src/builtins/fn_def.rs)          |
| `FN :{<schema>} -> <Type> = <body>`      | Anonymous function: a keyword-less record-schema binder. Registers no dispatch keyword — the returned value is the only handle (bind with `LET`, call by record `f {x = 1}`). | [fn_def.rs](src/builtins/fn_def.rs)          |
| `UNION <Name> = (<schema>)`              | Declare a tagged-union type. Binds `<Name>` in scope.                                            | [union.rs](src/builtins/union.rs)                    |
| `NEWTYPE <Name> = <Repr>`                | Declare a fresh nominal identity over a transparent representation — a scalar (`Number`) or a record (`:{x :Number, y :Number}`, the ex-`STRUCT` shape). `(Name value)` / `(Name {fields})` constructs. | [newtype_def.rs](src/builtins/newtype_def.rs)        |
| `MATCH <value> WITH (<branches>)` | Branch by tag; only the matching branch's body runs. `it` binds the inner value.                | [match_case.rs](src/builtins/match_case.rs)          |
| `TRY (<expr>) WITH (<branches>)`         | Evaluate `<expr>` in a catching context; branch on `ok` / the `KErrorKind` tags / `_`. `it` is the value (success) or per-variant payload (error). | [try_with.rs](src/builtins/try_with.rs)              |
| `<verb:Type> (<args>)`                   | Construct a tagged or newtype value, e.g. `Maybe (Some 42)` or `Point {x = 3, y = 4}`.            | [dispatch/single_poll.rs](src/machine/execute/dispatch/single_poll.rs) (`TypeCall` fast lane) |
| `<verb:Identifier> (<args>)`             | Call a function, tagged-union type, or newtype bound under `<verb>`.                            | [dispatch/fn_value.rs](src/machine/execute/dispatch/fn_value.rs) (`FunctionValueCall` fast lane) |
| `<s>.<field>` (`ATTR <s> <field>`)       | Read `<field>` off a record-repr newtype value. Compound-token `.` operator; `s.x.y` chains.     | [attr.rs](src/builtins/attr.rs)                      |
| `(<fields>) FROM <r:{}>`                 | Project a record value to the named fields, e.g. `(x y) FROM r` — re-tags the carried type to `{x, y}` to pick one of two incomparable dispatch arms. | [record_projection.rs](src/builtins/record_projection.rs) |
| `<v:Identifier>` (single-part)           | Look up `<v>` in scope.                                                                         | [dispatch/single_poll.rs](src/machine/execute/dispatch/single_poll.rs) (`BareIdentifier` fast lane) |
| `<v>` (single-part literal/expr)         | Pass the value through (lets `(99)`, `("x")`, etc. dispatch as expressions).                    | [dispatch/single_poll.rs](src/machine/execute/dispatch/single_poll.rs) (`LiteralPassThrough` fast lane) |
| `#(<expr>)`                              | Quote: capture the body's AST as a `KExpression` value with no evaluation.                       | [quote.rs](src/builtins/quote.rs)                    |
| `$(<expr>)`                              | Eval: resolve `<expr>`; if the result is a `KExpression`, dispatch the captured AST.             | [eval.rs](src/builtins/eval.rs)                      |

## What's not in the language yet

Tracked in [the roadmap](roadmap/README.md):

- **No user-declarable traits.** `UNION` and `STRUCT` cover sum and product
  types and `.` reads fields off a struct value, but there's no syntax yet
  for declaring a trait. `KType` is otherwise a closed enum.
- **No arithmetic, comparison, or logical operators.** `1 + 1` doesn't parse
  as addition. The character-trigger registry only does syntactic desugaring.
- **No loops.** Recursion is the iteration model; tail calls collapse cleanly.

If a snippet doesn't behave the way you expect, the most likely cause is one
of the above.

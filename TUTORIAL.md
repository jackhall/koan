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
[`KError`](src/dispatch/kerror.rs) values printed to stderr.

## Source structure

A Koan source file is a list of top-level expressions. An expression is an
ordered sequence of *parts*: tokens, literals, and nested sub-expressions. The
parser produces one [`KExpression`](src/parse/kexpression.rs) per top-level line
and hands it to dispatch.

### Tokens

Every non-literal atom falls into one of three classes, decided by the casing
rule in [tokens.rs](src/parse/tokens.rs):

| Class        | Rule                                            | Examples                  |
|--------------|-------------------------------------------------|---------------------------|
| `Keyword`    | all-caps, no lowercase                          | `LET`, `THEN`, `=`, `->`  |
| `Type`       | first char uppercase **and** at least one lower | `Number`, `KFunction`     |
| `Identifier` | everything else (lowercase, snake_case)         | `x`, `greeting`, `my_var` |

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
when) to dispatch it. `IF ... THEN`, `MATCH ... WITH`, the body of `FN`, and
the schemas of `UNION` are the cases where laziness shows up today.

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
nested block. A bare identifier on its own is a name lookup: it produces
the value the name was bound to, or an `UnboundName` error if there is no
such binding.

```
LET msg = "hi"
PRINT msg          # prints "hi"
```

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

| Type      | What it is                                         | How to write a value                         |
|-----------|----------------------------------------------------|----------------------------------------------|
| `Number`  | 64-bit float                                       | `42`, `3.14`                                 |
| `Str`     | string                                             | `"hi"`, `'hi'`                               |
| `Bool`    | boolean                                            | `true`, `false`                              |
| `Null`    | the null value                                     | `null`                                       |
| `List`    | ordered sequence                                   | `[1, 2, 3]`                                  |
| `Dict`    | scalar-keyed map                                   | `{a: 1, b: 2}`                               |
| `Tagged`  | a value of a tagged union                          | `Maybe (some 42)` (see `UNION` below)        |
| `Any`     | wildcard — accepts any value                       | (used in annotations only)                   |

A type name appears wherever you annotate something: the return type on a
function (`-> Number`), the type of a tagged-union variant (`some: Number`).
You'll also see `KFunction` (the type of a function value), `KExpression`
(an unevaluated parenthesized expression carried as data), and `Tagged`
referenced in error messages — they're real types, but you rarely write
them yourself.

Per-parameter type annotations on user functions don't exist yet —
parameter slots accept any type. Use `Any` as a return type when you don't
want a runtime check.

## User-defined functions

`FN <signature> -> <ReturnType> = <body>` registers a function. The signature
is a parens-wrapped expression mixing fixed `Keyword` tokens (the dispatch
shape) and `Identifier` parameter slots. The body is a parens-wrapped
expression evaluated at call time.

```
FN (DOUBLE x) -> Number = (x)
FN (a SAID) -> Null = (PRINT a)         # infix-shaped — keyword in non-leading position
FN (FIRST x y) -> Null = (PRINT x)      # multiple params

DOUBLE 21        # → 21
"hi" SAID        # prints "hi"
FIRST "a" "b"    # prints "a"
```

The return type is **non-optional** and **enforced at runtime**. A body whose
result doesn't match the declared type fails with
`KErrorKind::TypeMismatch { arg: "<return>", … }`. Use `-> Any` to opt out.

A signature must contain at least one `Keyword` (the dispatch token); otherwise
it would shadow `value_lookup`/`value_pass`. Type-name parts inside a signature
are rejected — types live only in the `-> Type` slot.

`FN` returns the registered `KFunction`, so you can capture it as a value:

```
LET f = (FN (DOUBLE x) -> Number = (x))
f (21)           # → 21, via call_by_name
```

Free names in a body resolve through the FN's *captured* definition scope —
true lexical scoping, including for closures returned from another function's
body. Recursion is the iteration model; tail calls reuse the calling slot.

## Tagged unions

`UNION` declares a type whose values carry a *tag* and a payload. Two forms:

```
UNION Maybe = (some: Number none: Null)              # named — registers `Maybe`
LET maybe = (UNION (ok: Str err: Str))               # anonymous — bind the type to a name
```

A tag is a bare identifier; a type is a type-name token. The schema body is a
parens-wrapped sequence of `<tag>: <Type>` triples.

Construct a value by calling the type with a `(tag value)` pair:

```
LET m = (Maybe (some 42))
LET r = (maybe (err "boom"))
```

The lowercase form (`maybe`) works because `call_by_name` recognizes
`TaggedUnionType` as a callable and routes to the same constructor as the
type-token form (`Maybe`).

Pattern-match on the tag with `MATCH ... WITH`. The branches are
`<tag> -> <body>` triples. A trailing comma joins the next line into the
same group:

```
MATCH (m) WITH
  some -> (PRINT "got"),
  none -> (PRINT "no")
```

Only the matching branch's body is dispatched. Inside a branch, `it` is bound
to the inner value:

```
UNION Result = (ok: Str err: Str)
LET r = (Result (ok "all good"))
MATCH (r) WITH (ok -> (PRINT it) err -> (PRINT "failed"))
```

A non-exhaustive match (no branch for the actual tag) errors with
`KErrorKind::ShapeError`.

## Structs

`STRUCT` declares a record type — an ordered list of named fields, each with
a declared type. The form mirrors `UNION`:

```
STRUCT Point = (x: Number, y: Number)
STRUCT User = (id: Number, name: Str, active: Bool)
```

Field declaration order is part of the contract: construction is positional,
so the *i*-th value goes to the *i*-th field. The construction surface
mirrors tagged unions:

```
LET p = (Point (3 4))
LET u = (User (42 "alice" true))
```

Bare identifiers in the args list resolve through scope just like literals
do — no extra parens needed:

```
LET vx = 7
LET vy = 11
LET q = (Point (vx vy))
```

Wrong arity or wrong field-type errors at construction time:

```
LET bad = (Point ("oops" 4))
# error: type mismatch for argument 'x': expected Number, got Str
```

A struct's runtime type is `KType::Struct`; the schema itself is
`KType::Type` (shared with `TaggedUnionType`).

## Errors

Failures are first-class [`KError`](src/dispatch/kerror.rs) values with a
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
`ParseError`, `User`. There's no in-language try/catch yet — errors
short-circuit to the top level. Intentional `null` values (`IF false THEN x`,
`PRINT`'s return) are not errors.

## Putting it together

```
UNION Greeting = (formal: Str casual: Str)

FN (SAY msg) -> Null = (PRINT msg)

LET hello = (Greeting (casual "hey"))

MATCH (hello) WITH
  formal -> (SAY "greetings, sir"),
  casual -> (SAY it)
```

What runs:

1. `UNION Greeting = ...` registers a tagged-union type with two variants.
2. `FN (SAY msg) -> Null = (PRINT msg)` defines a one-arg function over
   strings.
3. `LET hello = (Greeting (casual "hey"))` builds a `Tagged` value with tag
   `casual` and payload `"hey"`, and binds it as `hello`.
4. `MATCH` sees the `casual` tag, runs the `casual` branch, and `SAY it`
   prints `hey`.

## Builtin reference

One line per surface form. Sources under
[src/dispatch/builtins/](src/dispatch/builtins/).

| Form                                                  | Effect                                                                                          | File                                                          |
|-------------------------------------------------------|-------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| `LET <name> = <value>`                                | Bind `<name>` to `<value>` in the current scope. Returns the bound value.                       | [let_binding.rs](src/dispatch/builtins/let_binding.rs)        |
| `PRINT <msg:Str>`                                     | Write `<msg>` and a newline to the scope's output sink. Returns null.                           | [print.rs](src/dispatch/builtins/print.rs)                    |
| `IF <pred:Bool> THEN <expr>`                          | Lazy: dispatch `<expr>` only when `<pred>` is true. Wrap `<expr>` in parens.                    | [if_then.rs](src/dispatch/builtins/if_then.rs)                |
| `FN <sig> -> <Type> = <body>`                         | Register a user function with signature `<sig>` and runtime-enforced return type. Returns the function. | [fn_def.rs](src/dispatch/builtins/fn_def.rs)          |
| `UNION <Name> = (<schema>)` / `UNION (<schema>)`      | Declare a tagged-union type. Named form binds `<Name>` in scope.                                | [union.rs](src/dispatch/builtins/union.rs)                    |
| `STRUCT <Name> = (<schema>)`                          | Declare a record type with ordered, typed fields. Binds `<Name>` in scope.                       | [struct_def.rs](src/dispatch/builtins/struct_def.rs)          |
| `MATCH <value:Tagged> WITH (<branches>)`              | Branch by tag; only the matching branch's body runs. `it` binds the inner value.                | [match_case.rs](src/dispatch/builtins/match_case.rs)          |
| `<verb:TypeRef> (<args>)`                             | Construct a tagged or struct value, e.g. `Maybe (some 42)` or `Point (3 4)`.                    | [type_call.rs](src/dispatch/builtins/type_call.rs)            |
| `<verb:Identifier> (<args>)`                          | Call a function, tagged-union type, or struct type bound under `<verb>`.                        | [call_by_name.rs](src/dispatch/builtins/call_by_name.rs)      |
| `<v:Identifier>` (single-part)                        | Look up `<v>` in scope.                                                                         | [value_lookup.rs](src/dispatch/builtins/value_lookup.rs)      |
| `<v>` (single-part literal/expr)                      | Pass the value through (lets `(99)`, `("x")`, etc. dispatch as expressions).                    | [value_pass.rs](src/dispatch/builtins/value_pass.rs)          |

## What's not in the language yet

Tracked in [ROADMAP.md](ROADMAP.md):

- **No user-declarable traits, no field access on structs.** `UNION` and
  `STRUCT` cover sum and product types, but there's no syntax yet for
  reading a field off a struct value or for declaring a trait. `KType` is
  otherwise a closed enum.
- **No per-parameter type annotations** on user functions (uniformly `Any`).
- **No arithmetic, comparison, or logical operators.** `1 + 1` doesn't parse
  as addition. The character-trigger registry only does syntactic desugaring.
- **No loops.** Recursion is the iteration model; tail calls collapse cleanly.
- **No in-language error catching.** Errors propagate to the CLI.

If a snippet doesn't behave the way you expect, the most likely cause is one
of the above.

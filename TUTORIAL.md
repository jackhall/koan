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
[`KError`](src/dispatch/runtime/kerror.rs) values printed to stderr.

## Source structure

A Koan source file is a list of top-level expressions. An expression is an
ordered sequence of *parts*: tokens, literals, and nested sub-expressions. The
parser produces one [`KExpression`](src/parse/kexpression.rs) per top-level line
and hands it to dispatch.

### Tokens

Every non-literal atom falls into one of three classes, decided by the casing
rule in [tokens.rs](src/parse/tokens.rs):

| Class        | Rule                                                                      | Examples                       |
|--------------|---------------------------------------------------------------------------|--------------------------------|
| `Keyword`    | pure-symbol, **or** alphabetic with ≥2 uppercase letters and no lowercase | `=`, `->`, `:|`, `LET`, `THEN` |
| `Type`       | first char uppercase **and** at least one lowercase elsewhere             | `Number`, `KFunction`, `IntOrd` |
| `Identifier` | lowercase- or `_`-leading                                                 | `x`, `greeting`, `my_var`      |

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

A name lookup whose binder hasn't run yet — a forward reference between
sibling top-level expressions — *parks* on the producer rather than
failing. Once the binder finishes, the consumer wakes and resumes:

```
LET y = x
LET x = 42
PRINT y            # prints 42
```

A name that no binder ever introduces still surfaces as `UnboundName`.
Re-binding a value name in the same scope surfaces as `Rebind`; shadowing
across nested scopes (a child block, a function body) is allowed and is
how lexical scoping works.

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

| Type                       | What it is                          | How to write a value                  |
|----------------------------|-------------------------------------|---------------------------------------|
| `Number`                   | 64-bit float                        | `42`, `3.14`                          |
| `Str`                      | string                              | `"hi"`, `'hi'`                        |
| `Bool`                     | boolean                             | `true`, `false`                       |
| `Null`                     | the null value                      | `null`                                |
| `List<T>`                  | ordered sequence                    | `[1, 2, 3]`                           |
| `Dict<K, V>`               | scalar-keyed map                    | `{a: 1, b: 2}`                        |
| `Function<(args) -> R>`    | callable function value             | `(FN (DOUBLE x: Number) -> Number = (x))` |
| `Tagged`                   | a value of a tagged union           | `Maybe (some 42)` (see `UNION` below) |
| `Any`                      | wildcard — accepts any value        | (used in annotations only)            |

A type name appears wherever you annotate something: the type of a parameter
slot (`x: List<Number>`), the return type on a function (`-> Number`), the
type of a tagged-union variant (`some: Number`). Container types are always
parameterized — bare `List` lowers to `List<Any>`, bare `Dict` to
`Dict<Any, Any>`. There is no bare `Function`; write
`Function<(args) -> R>` for a typed function or `Any` for an unconstrained
value (a function with no signature has nothing to dispatch on).

You'll also see `KExpression` (an unevaluated parenthesized expression carried
as data) referenced in builtin signatures and error messages — it's a real
type, but you rarely write it yourself. List/dict literal types are inferred
as the join of element types: `[1, 2, 3]` is `List<Number>`, `[1, "x"]` is
`List<Any>`, `[]` is `List<Any>`.

## User-defined functions

`FN <signature> -> <ReturnType> = <body>` registers a function. The signature
is a parens-wrapped expression mixing fixed `Keyword` tokens (the dispatch
shape) and typed parameter slots written as `name: Type`. The body is a
parens-wrapped expression evaluated at call time.

```
FN (DOUBLE x: Number) -> Number = (x)
FN (a: Str SAID) -> Null = (PRINT a)            # infix-shaped — keyword in non-leading position
FN (FIRST x: Str y: Str) -> Null = (PRINT x)    # multiple params
FN (ADD x: Number, y: Number) -> Number = (x)   # commas optional, same shape
FN (HEAD xs: List<Number>) -> Number = (1)      # parameterized container in a slot
FN (NUMS) -> List<Number> = ([1 2 3])           # parameterized return type

DOUBLE 21        # → 21
"hi" SAID        # prints "hi"
FIRST "a" "b"    # prints "a"
```

Both the parameter types and the return type are **non-optional**. A bare
`x` without `: Type` is a parse error. Calls whose argument types don't satisfy
the signature fail at dispatch (`KErrorKind::DispatchFailed`); the same call
shape with different parameter types routes to a different overload by
slot-specificity (more specific wins — `List<Number>` beats `List<Any>` beats
`Any`). Use `: Any` to opt a slot out of type checking.

The return type is **enforced at runtime**. A body whose result doesn't match
the declared type fails with `KErrorKind::TypeMismatch { arg: "<return>", … }`.
For parameterized container returns, the check walks elements: a function
declared `-> List<Number>` whose body returns `[1, "x"]` errors with
`expected List<Number>, got List<Any>`. Use `-> Any` to opt out.

A signature must contain at least one `Keyword` (the dispatch token); otherwise
it would shadow `value_lookup`/`value_pass`.

`FN` returns the registered function value, so you can capture it as a value:

```
LET f = (FN (DOUBLE x: Number) -> Number = (x))
f (x: 21)        # → 21, via call_by_name (named arguments)
```

Function calls through `call_by_name` use **named arguments**: each value is
introduced by its parameter name and a colon. Order is independent of the
declaration:

```
LET pair = (FN (a: Number TIMES b: Number) -> Number = (a))
pair (a: 3, b: 4)        # → 3
pair (b: 4, a: 3)        # → 3 (same call, different argument order)
```

Missing names error with `KErrorKind::MissingArg`; unknown names with
`KErrorKind::ShapeError`.

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

Construction is **named**: each value is introduced by its field name and a
colon. Order is independent of the declaration — the constructor reorders the
pairs into schema order before validating types:

```
LET p = (Point (x: 3, y: 4))
LET u = (User (id: 42, name: "alice", active: true))
LET q = (Point (y: 4, x: 3))             # same struct as p
```

Bare identifiers on the value side resolve through scope just like literals do —
no extra parens needed:

```
LET vx = 7
LET vy = 11
LET q = (Point (x: vx, y: vy))
```

Missing or unknown field names, and wrong field-type values, all error at
construction time:

```
LET bad = (Point (x: "oops", y: 4))
# error: type mismatch for argument 'x': expected Number, got Str

LET partial = (Point (x: 3))
# error: missing argument 'y'
```

A struct's runtime type is `KType::Struct`; the schema itself is
`KType::Type` (shared with `TaggedUnionType`).

Read a field off a struct value with the `.` operator (an alias for the
`ATTR` builtin):

```
LET dx = p.x                             # 3
PRINT (p.y)                              # 4

STRUCT Line = (start: Struct, finish: Struct)
LET seg = (Line (start: p, finish: q))
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

Failures are first-class [`KError`](src/dispatch/runtime/kerror.rs) values with a
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
matches an already-registered overload), `User`. There's no in-language
try/catch yet — errors short-circuit to the top level. Intentional `null`
values (the `null` literal, `PRINT`'s return) are not errors.

## Putting it together

```
UNION Greeting = (formal: Str casual: Str)

FN (SAY msg: Str) -> Null = (PRINT msg)

LET hello = (Greeting (casual "hey"))

MATCH (hello) WITH
  formal -> (SAY "greetings, sir"),
  casual -> (SAY it)
```

What runs:

1. `UNION Greeting = ...` registers a tagged-union type with two variants.
2. `FN (SAY msg: Str) -> Null = (PRINT msg)` defines a one-arg function over
   strings.
3. `LET hello = (Greeting (casual "hey"))` builds a `Tagged` value with tag
   `casual` and payload `"hey"`, and binds it as `hello`.
4. `MATCH` sees the `casual` tag, runs the `casual` branch, and `SAY it`
   prints `hey`.

## Builtin reference

One line per surface form. Sources under
[src/builtins/](src/builtins/).

| Form                                                  | Effect                                                                                          | File                                                          |
|-------------------------------------------------------|-------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| `LET <name> = <value>`                                | Bind `<name>` to `<value>` in the current scope. Returns the bound value.                       | [let_binding.rs](src/builtins/let_binding.rs)        |
| `PRINT <msg:Str>`                                     | Write `<msg>` and a newline to the scope's output sink. Returns null.                           | [print.rs](src/builtins/print.rs)                    |
| `FN <sig> -> <Type> = <body>`                         | Register a user function. Parameter slots in `<sig>` are typed (`name: Type`); the return type is runtime-enforced. Returns the function. | [fn_def.rs](src/builtins/fn_def.rs)          |
| `UNION <Name> = (<schema>)` / `UNION (<schema>)`      | Declare a tagged-union type. Named form binds `<Name>` in scope.                                | [union.rs](src/builtins/union.rs)                    |
| `STRUCT <Name> = (<schema>)`                          | Declare a record type with ordered, typed fields. Binds `<Name>` in scope.                       | [struct_def.rs](src/builtins/struct_def.rs)          |
| `MATCH <value:Tagged> WITH (<branches>)`              | Branch by tag; only the matching branch's body runs. `it` binds the inner value.                | [match_case.rs](src/builtins/match_case.rs)          |
| `<verb:TypeExprRef> (<args>)`                         | Construct a tagged or struct value, e.g. `Maybe (some 42)` or `Point (x: 3, y: 4)`.             | [type_call.rs](src/builtins/type_call.rs)            |
| `<verb:Identifier> (<args>)`                          | Call a function, tagged-union type, or struct type bound under `<verb>`.                        | [call_by_name.rs](src/builtins/call_by_name.rs)      |
| `<s>.<field>` (`ATTR <s> <field>`)                    | Read `<field>` off a struct value. Compound-token `.` operator; `s.x.y` chains.                  | [attr.rs](src/builtins/attr.rs)                      |
| `<v:Identifier>` (single-part)                        | Look up `<v>` in scope.                                                                         | [value_lookup.rs](src/builtins/value_lookup.rs)      |
| `<v>` (single-part literal/expr)                      | Pass the value through (lets `(99)`, `("x")`, etc. dispatch as expressions).                    | [value_pass.rs](src/builtins/value_pass.rs)          |
| `#(<expr>)`                                           | Quote: capture the body's AST as a `KExpression` value with no evaluation.                       | [quote.rs](src/builtins/quote.rs)                    |
| `$(<expr>)`                                           | Eval: resolve `<expr>`; if the result is a `KExpression`, dispatch the captured AST.             | [eval.rs](src/builtins/eval.rs)                      |

## What's not in the language yet

Tracked in [ROADMAP.md](ROADMAP.md):

- **No user-declarable traits.** `UNION` and `STRUCT` cover sum and product
  types and `.` reads fields off a struct value, but there's no syntax yet
  for declaring a trait. `KType` is otherwise a closed enum.
- **No arithmetic, comparison, or logical operators.** `1 + 1` doesn't parse
  as addition. The character-trigger registry only does syntactic desugaring.
- **No loops.** Recursion is the iteration model; tail calls collapse cleanly.
- **No in-language error catching.** Errors propagate to the CLI.

If a snippet doesn't behave the way you expect, the most likely cause is one
of the above.

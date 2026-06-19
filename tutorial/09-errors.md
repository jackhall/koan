# Errors

When something goes wrong, Koan raises a structured **error value**. An error
carries a *kind* (what went wrong) and a chain of *frames* (where it came from).
An uncaught error stops the program and prints to standard error, listing the
frames beneath the message:

```koan
FN (BOOM x :Number) -> Str = (mystery)
BOOM 1
```

```text
error: unbound name 'mystery'
  in fn(BOOM <x>) (fn(BOOM <x>))
```

The `in …` lines are the call trace, innermost first. An error raised at the top
level, outside any call, has no frames.

## Catching errors with `TRY`

`TRY (<expr>) -> :<Type> WITH (<branches>)` runs `<expr>` in a catching context
and dispatches to a branch based on the result. Like `MATCH`, it needs a result
type, and every branch must produce that type. The branch tags are:

- `Ok` — the expression succeeded; `it` is its value.
- an **error-kind tag** — the matching error was caught; `it` is the error's
  payload.
- `_` — a wildcard catching anything not named.

```koan
TRY (PRINT "working") -> :Str WITH (Ok -> (PRINT "all good"))
```

```text
working
all good
```

When an error is caught, `it` holds a payload with fields describing the error.
An `UnboundName` carries the offending name as `it.name`:

```koan
TRY (mystery) -> :Str WITH
  Ok -> (PRINT "ok"),
  UnboundName -> (PRINT it.name)
```

```text
mystery
```

An exact-tag branch always wins over `_`, regardless of order, so you can handle
specific kinds and let the wildcard mop up the rest:

```koan
TRY (mystery) -> :Str WITH
  TypeMismatch -> (PRINT "type problem"),
  _ -> (PRINT "something failed")
```

```text
something failed
```

Because the whole `TRY` produces a value of its result type, it's a clean way to
supply a fallback:

```koan
LET safe =
  TRY (mystery) -> :Str WITH
    Ok -> (it),
    _ -> ("default")
PRINT safe
```

```text
default
```

If a caught error has no matching branch and there's no `_`, the original error
is re-raised. If the expression *succeeds* but there's no `Ok` branch, that's a
`shape error`.

### The error kinds you can catch

Each error-kind tag carries its own payload fields, all reachable through `it`.
The kinds you'll meet most are unbound names and failed dispatch:

| Tag                 | Meaning                                  | Payload fields                |
|---------------------|------------------------------------------|-------------------------------|
| `UnboundName`       | a name with no binding                   | `it.name`                     |
| `DispatchFailed`    | no function matched the expression's shape | `it.expr`, `it.reason`      |
| `TypeMismatch`      | a value's type didn't match what was required | `it.arg`, `it.expected`, `it.got` |
| `MissingArg`        | a required named argument was absent     | `it.name`                     |
| `ArityMismatch`     | wrong number of arguments                | `it.expected`, `it.got`       |
| `AmbiguousDispatch` | more than one function matched equally    | `it.expr`, `it.candidates`    |
| `ShapeError`        | a structural rule was violated            | `it.message`                  |
| `ParseError`        | the source didn't parse                   | `it.message`                  |

Every error arm's `it` also carries `it.frames` — the call trace as a list of
strings.

## Turning errors into values with `CATCH`

`TRY` branches immediately. Sometimes you'd rather capture an outcome as a
*value* and keep going. `CATCH (<expr>)` runs the expression and returns a
[`Result`](#result): `Ok` with the value on success, `Error` with the payload on
failure — without stopping the program:

```koan
PRINT (CATCH (PRINT "hi"))
```

```text
hi
Ok(hi)
```

The inner `PRINT` runs and returns `"hi"`, which `CATCH` wraps as `Ok`. On
failure you get an `Error` carrying the error's payload. Rather than print that
raw value, you typically [`MATCH`](06-pattern-matching.md) on the result:

```koan
MATCH (CATCH (mystery)) -> :Str WITH
  Ok -> (PRINT "succeeded"),
  Error -> (PRINT "caught a failure")
```

```text
caught a failure
```

## Result

`Result` is a built-in tagged union with two variants, `Ok` and `Error`,
available without declaring it. Its error variant is spelled `Error`, not `Err`:

```koan
PRINT (Result (Ok 1))
PRINT (Result (Error "boom"))
```

```text
Ok(1)
Error(boom)
```

## Branch scoping

The `TRY` body and each branch are their own scopes. A name bound inside a
branch is local to it and gone afterward:

```koan
TRY (PRINT "x") -> :Str WITH (Ok -> ((LET note = "local") (PRINT note)))
note
```

```text
x
local
error: unbound name 'note'
```

Next: [Quoting and evaluating](10-quoting.md).

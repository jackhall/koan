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
  in :(FN :{x :Number} -> Str) (BOOM 1) at <input>:2:1
```

The `in …` lines are the call trace, innermost first. A frame for a function call
names it two ways — the function's signature type, then the call site's own source
text in parentheses — and ends with the file, line and column that call sits at.
An error raised at the top level, outside any call, has no frames.

## Catching errors with `TRY`

`TRY (<expr>) -> :<Type> WITH (<branches>)` runs `<expr>` in a catching context
and dispatches to a branch based on the result. Like `MATCH`, it needs a result
type, and every branch must produce that type. The branch heads are:

- `Ok` — the expression succeeded; `it` is its value.
- an **error-kind name** — the matching error was caught; `it` is the error's
  payload record.
- `_` — a default branch catching every kind you did not name.

Unlike `MATCH … OVER`, a `TRY` need not cover every kind: an error no branch
names is simply re-raised, so `_` is optional. Anything else in head position —
a boolean literal, a name that is not an error kind — is an error at the form.

```koan
TRY (PRINT "working") -> :Str WITH (Ok -> (PRINT "all good"))
```

```text
working
all good
```

When an error is caught, `it` holds a record of fields describing the error. You
can print it whole:

```koan
TRY (mystery) -> :Str WITH
  Ok -> (PRINT "ok"),
  UnboundName -> (PRINT it)
```

```text
{frames = [], name = mystery}
```

It is an ordinary record, so you can read a single field off it:

```koan
TRY (mystery) -> :Str WITH
  Ok -> (PRINT "ok"),
  UnboundName -> (PRINT it.name)
```

```text
mystery
```

Every kind's record carries `frames`, the call stack the error passed through;
[the error kinds you can catch](#the-error-kinds-you-can-catch) lists the rest per kind.

A named branch always wins over `_`, regardless of order, so you can handle
specific kinds and let the default mop up the rest:

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

Each error kind carries its own payload fields, which `it` binds as a record.
The kinds you'll meet most are unbound names and failed dispatch:

| Kind                | Meaning                                  | Payload fields                |
|---------------------|------------------------------------------|-------------------------------|
| `UnboundName`       | a name with no binding                   | `it.name`                     |
| `DispatchFailed`    | no function matched the expression's shape | `it.expr`, `it.reason`      |
| `TypeMismatch`      | a value's type didn't match what was required | `it.arg`, `it.expected`, `it.got` |
| `MissingArg`        | a required named argument was absent     | `it.name`                     |
| `ArityMismatch`     | wrong number of arguments                | `it.expected`, `it.got`       |
| `AmbiguousDispatch` | more than one function matched equally    | `it.expr`, `it.candidates`    |
| `ShapeError`        | a structural rule was violated            | `it.message`                  |
| `ParseError`        | the source didn't parse                   | `it.message`                  |

Every error branch's `it` also carries a `frames` field — the call trace as a
list of strings.

The dispatcher-internal kinds (`Rebind`, `DuplicateDeclaration`,
`DuplicateOverload`, `TypeClassBindingExpectsType`, `SchedulerDeadlock`) are
catchable by name too, with a flattened `{kind, message, frames}` payload; `_`
reaches them like any other kind you did not name.

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
failure you get an `Error` carrying the error's payload instead, and the
program keeps running:

```koan
LET outcome = (CATCH (mystery))
PRINT "still going"
```

```text
still going
```

Use `CATCH` when you want to hold the outcome as a value and hand it on; use
`TRY` when you want to branch on it right away.

## Result

`Result` is a built-in union with two variants, `Ok` and `Error`, available
without declaring it. Its error variant is spelled `Error`, not `Err`. Like any
union, you reach a variant by projecting it off the union's name:

```koan
PRINT (Result.Ok 1)
PRINT (Result.Error "boom")
```

```text
Ok(1)
Error(boom)
```

And like any union, you take one apart with `MATCH … OVER`:

```koan
MATCH (CATCH (mystery)) OVER Result -> :Str WITH
  Ok -> (PRINT "worked"),
  Error -> (PRINT "failed")
```

```text
failed
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

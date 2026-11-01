# Errors

When something goes wrong while a program runs, Koan raises an **error**. An
error is not a value: nothing receives it and carries on. It leaves the
expression that raised it, and every call that expression sits in, until
something catches it, and the parts of an expression after the one that failed
never run. An uncaught error stops the program and prints its message to
standard error, and beneath it the **trace**: one line per call the error came
out of, innermost first, each naming the function's type, the call as you wrote
it, and where the call is:

```koan
EXPR #(BOOM p :Any) -> Number = #(p.z + 1)
PRINT "before"
PRINT (BOOM {x = 1})
PRINT "after"
```

```text
before
error: :{x :Number} has no field z
  in :(FN :{p :Any} -> Number) (BOOM {x = 1}) at <input>:3:7
```

An error raised outside any call prints no trace line. A call that runs many
times from one place — a function looping through calls to itself — prints one
line with a count, such as `×1000`, rather than a line per call.

Some mistakes need no run to be found. A call that no overload can ever accept,
a field read the record's type rules out, or a function body that can never
produce its declared return type, is refused when the program loads, located at
its line and column, before anything runs — so `before` is never printed:

```koan
PRINT "before"
PRINT (1 + "one")
```

```text
error: <input>:2:7: no overload of `_ + _` admits these arguments:
  Str, which can never satisfy Number
```

## Catching errors with `TRY`

`TRY <body> WITH #{<arms>}` runs its body, a parenthesized block. If the body
produces a value, that value is the `TRY`'s. If it raises an error, the `TRY`
picks an arm by the error's **kind** and runs it instead, its value standing in
for the body's. The arms are a dict of quotes, as a `MATCH`'s are, and each
head is:

- an **error-kind name**, such as `NoField` — the arm for that kind; `it` is a
  record describing the error.
- `_` — the arm for every kind no other head names.

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
PRINT (TRY (GRAB {x = 1}) WITH #{NoField: ("no z here")})
PRINT (TRY (GRAB {z = 2}) WITH #{NoField: ("no z here")})
```

```text
no z here
2
```

A head that names no error kind is refused when the program loads.

### What `it` holds

A caught error's `it` is an ordinary record. Every kind's record has
`message`, the text an uncaught error prints after `error: `, and `frames`, the
lines its trace prints, innermost first, each a record of the callee's type,
the call as written, where it is, and how many calls the line counts. Beside
those, each kind has fields of its own: a `NoField` holds `of`, the value that
lacked the field, and `field`, the field's name.

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
TRY (GRAB {x = 1}) WITH #{NoField: (PRINT it)}
```

```text
{field = z, frames = [{at = <input>:2:5, call = GRAB {x = 1}, callee = :(FN :{p :Any} -> Any), count = 1}], message = :{x :Number} has no field z, of = {x = 1}}
```

Read one field off it as off any record:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
TRY (GRAB {x = 1}) WITH #{NoField: ((PRINT it.message) (PRINT it.of))}
```

```text
:{x :Number} has no field z
{x = 1}
```

The values an error names are the values the failing code held, so `it.of`
above is the record itself, not its type. `frames` lists every call between
the error and the `TRY`:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
EXPR #(OUTER p :Any) -> Any = #((LET got = (GRAB p)) (got))
TRY (OUTER {x = 1}) WITH #{NoField: (PRINT it.frames)}
```

```text
[{at = <input>:2:44, call = GRAB p, callee = :(FN :{p :Any} -> Any), count = 1}, {at = <input>:3:5, call = OUTER {x = 1}, callee = :(FN :{p :Any} -> Any), count = 1}]
```

[The error kinds a running program meets](#the-error-kinds-a-running-program-meets)
lists the ones you will catch most often, with their fields.

### Choosing an arm

A named arm always wins over `_`, whatever their order, so you can handle the
kinds you expect and let `_` take the rest:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
EXPR #(SIZE p :Any) -> Any = #(MATCH p WITH #{Number: (it), Str: (LENGTH it)})
PRINT (TRY (SIZE true) WITH #{
  NoField: ("a field")
  _: ("something else")
})
PRINT (TRY (GRAB 1) WITH #{
  _: ("something else")
  NoField: ("a field")
})
```

```text
something else
a field
```

A `TRY` need not name every kind. An error of a kind none of its arms takes is
not caught: it leaves the `TRY` as it came, trace and all, as if the `TRY` were
not there:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
PRINT (TRY (GRAB {x = 1}) WITH #{NoArm: ("no arm")})
PRINT "after"
```

```text
error: :{x :Number} has no field z
  in :(FN :{p :Any} -> Any) (GRAB {x = 1}) at <input>:2:12
```

An error raised inside an arm is not caught by that arm's `TRY`; it leaves the
`TRY` like any other error.

Because the `TRY` produces whichever value its body or its arm does, it is a
clean way to supply a fallback:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
LET safe = (TRY (GRAB {x = 1}) WITH #{_: ("default")})
PRINT safe
```

```text
default
```

Its type spans the body's and the arms', as a `MATCH`'s spans its arms'. To
hold it to one type, ascribe it: `(TRY … WITH …) :! :Str`.

### The error kinds a running program meets

| Kind           | Raised when                                        | Fields beside `message` and `frames` |
|----------------|----------------------------------------------------|--------------------------------------|
| `NoField`      | a record has no field of the name read             | `of`, `field`                        |
| `NoOverload`   | no overload admits a call's arguments              | `key`, `arguments`                   |
| `NoArm`        | no arm of a `MATCH` admits its value               | `scrutinee`, `union`                 |
| `Incomparable` | two values cannot be compared                      | `left`, `right`                      |
| `Returned`     | a function returns a value outside its return type | `callee`, `value`, `returns`         |
| `Unascribable` | a value does not satisfy its `:!` ascription       | `value`, `ascribed`                  |

Most of these the load already refuses wherever it can see the types; at run
time they come from values whose type is `Any`, or a union, until they arrive:

```koan
EXPR #(HALF n :Any) -> Number = #(n)
PRINT (TRY (HALF "two") WITH #{Returned: (it.message)})
LET pair = ({k = 1} :! :{k :Any})
PRINT (TRY (MATCH pair.k WITH #{Str: (it)}) WITH #{NoArm: (it.scrutinee)})
```

```text
:(FN :{n :Any} -> Number) returned Str, which does not satisfy Number
1
```

Every kind is a variant of the built-in union `Error`, named as the kind is.

## Turning errors into values with `CATCH`

`TRY` branches at once. Sometimes you would rather keep the outcome as a value
and decide later. `CATCH <body>` runs its body and produces a
[`Result`](#result): `Result.Ok` of the value when the body succeeds, and
`Result.Error` of the error when it fails, the error being its `Error` variant
over the same record a `TRY`'s `it` holds:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
PRINT (CATCH (GRAB {z = 2}))
PRINT (CATCH (GRAB {x = 1}))
```

```text
:(Ok {Error = Never, Ok = Number})(2)
:(Error {Error = NoField, Ok = Never})(NoField({field = z, frames = [{at = <input>:3:14, call = GRAB {x = 1}, callee = :(FN :{p :Any} -> Any), count = 1}], message = :{x :Number} has no field z, of = {x = 1}}))
```

`CATCH` takes every kind. Use it when you want to hold the outcome and hand it
on; use `TRY` when you want to branch on it right away.

## Result

`Result` is a built-in union with two variants, `Ok` and `Error`, available
without declaring it. Like any union, you reach a variant by projecting it off
the union's name:

```koan
PRINT (Result.Ok 1)
PRINT (Result.Error "boom")
```

```text
:(Ok {Error = Never, Ok = Number})(1)
:(Error {Error = Str, Ok = Never})(boom)
```

And like any union, you take one apart with `MATCH … UNDER`. A `CATCH`'s error
is a value of `Error`, which you take apart the same way:

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
LET outcome = (CATCH (GRAB {x = 1}))
PRINT (MATCH outcome UNDER Result WITH #{
  Ok: ("worked")
  Error: (MATCH it UNDER Error WITH #{NoField: (it.message), _: ("other")})
})
```

```text
:{x :Number} has no field z
```

## Scoping

A `TRY` or `CATCH` body is a block, and so is each arm: a name bound inside one
is local to it and gone afterward.

```koan
EXPR #(GRAB p :Any) -> Any = #(p.z)
TRY ((LET inside = 1) (GRAB {x = inside})) WITH #{_: ((LET note = "local") (PRINT note))}
PRINT note
```

```text
error: <input>:3:7: `note` names no binding visible here
```

Next: [Quoting and evaluating](10-quoting.md).

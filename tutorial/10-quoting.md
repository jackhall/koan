# Quoting and evaluating

Normally an argument is evaluated before the expression around it runs. Two
prefix sigils let you override that: capture an expression as a value without
running it, and run such a captured expression later.

## Quoting with `#`

`#(<expr>)` *quotes*: it captures the parenthesized expression as a value
without evaluating it. The captured expression is a value like any other — you
can bind it, pass it, and store it — and nothing inside it runs until you ask:

```koan
LET action = #(PRINT "hi")
PRINT "nothing ran yet"
```

```text
nothing ran yet
```

The `PRINT "hi"` never executed; `action` just holds it as data.

## Evaluating with `$`

`$(<expr>)` *evaluates*: it takes a value, and if that value is a quoted
expression, runs it in the current scope. Pairing the two, the captured action
runs only when evaluated:

```koan
LET action = #(PRINT "hi")
PRINT "about to run it"
$(action)
```

```text
about to run it
hi
```

Together, `#` and `$` let you move a piece of unevaluated code through positions
that would otherwise run it eagerly, and run it where you choose. Evaluating a
value that *isn't* a quoted expression is an error:

```koan
LET n = 5
$(n)
```

```text
error: type mismatch for argument 'expr': expected KExpression, got Number
```

## Passing code to a function you wrote

Every argument evaluates before the call it belongs to. That is the whole rule,
and it holds for the functions you define as much as for the built-in ones. So
a function that wants *code* rather than a result declares a parameter typed
`:KExpression` and is called with a quote:

```koan
FN (TWICE body :KExpression) -> Any = (
  $(body)
  $(body)
)
TWICE #(PRINT "hi")
```

```text
hi
hi
```

`body` receives the quoted expression as a value; each `$(body)` runs it. Any
expression that produces a quoted-expression value fills the slot just as well
— a name bound to a quote, or a call that returns one — because the slot takes
a value, not a spelling.

Forget the `#` and the argument is an ordinary group, so it runs before
`TWICE` is ever chosen. Writing

```koan
TWICE (PRINT "hi")
```

prints `hi` once — that is the argument evaluating — and *then* fails to
dispatch, because what reached the slot was the `Str` the print returned, not
code. Nothing is undone by the failure; the side effect had already happened.
The error names the missing quote:

```koan
FN (TWICE body :KExpression) -> Any = (
  $(body)
  $(body)
)
LET greeting = "hi"
TWICE greeting
```

```text
error: dispatch failed for TWICE Str at <input>:6:1: no matching function: an argument evaluated before dispatch; write #(…) to pass the code itself
```

The diagnostic names each argument by the *type* dispatch matched it on, not by
its spelling — `greeting` had already evaluated to a `Str`, and a `Str` is what
failed to match a `:KExpression` slot. The site after the expression is where to
read the spelling back.

Hence the rule for calling a form that takes code: **quote what must not run.**

The built-in forms that *do* take a bare body — the branches of a
[`MATCH`](06-pattern-matching.md), the block of a `TRY`, the body of an `FN` —
are fixed syntax, and their unevaluated slots are a closed list you cannot add
to. Your own forms take code the one way: as a `#(…)` value at every call site,
which is also what makes it visible to a reader that the group does not run
there.

## The sigil must be glued

Each sigil and its opening parenthesis are a single unit — the `(` must come
immediately after the `#` or `$`, with no space:

```koan
LET action = # (1)
```

```text
error: parse error: expected '(' after '#', found ' '
```

Next: [Modules](11-modules.md).

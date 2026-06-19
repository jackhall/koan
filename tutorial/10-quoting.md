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

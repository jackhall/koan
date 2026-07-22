# Getting started

Koan is a small, expression-oriented language. You write a program as a
sequence of expressions, and Koan runs them top to bottom. There are no
statements, no semicolons, and no `main` function — every line is an
expression that produces a value.

This chapter covers how to run a program and how source is structured. The
rest of the tutorial builds up the language feature by feature; see the
[README](README.md) for the full reading order.

## Running a program

The `koan` command reads source from a file argument or from standard input:

```sh
koan hello.koan
echo 'PRINT "hello"' | koan
```

From a source checkout, `cargo run -- hello.koan` is the equivalent.

A program is a sequence of top-level expressions, one per logical line, all
run in submission order against the same scope. The simplest program prints a
string:

```koan
PRINT "hello"
```

```text
hello
```

Each line runs in order, and later lines can see what earlier lines bound:

```koan
LET name = "world"
PRINT name
```

```text
world
```

Anything that goes wrong surfaces as a structured error printed to standard
error, and the program stops. For example, referring to a name that was never
bound:

```koan
PRINT mystery
```

```text
error: unbound name 'mystery'
```

## Expressions and grouping

An expression is a sequence of *parts*: keywords, names, literals, and nested
sub-expressions. Parentheses group a run of parts into a nested expression
that runs on its own, with its result handed back to the enclosing
expression. So the inner addition here runs first, and its sum becomes what
`PRINT` receives:

```koan
PRINT (1 + 2)
```

```text
3
```

Declarations are the exception: a form that binds a name, like `LET`, stands
as its own line rather than riding inside another expression's arguments —
[chapter 3](03-names-and-dispatch.md) covers exactly where declarations may
appear.

### Indentation is grouping

Two-space indentation under a line means the same thing as wrapping the
indented part in parentheses. These two programs are identical:

```koan
PRINT (1 + 2)
```

```koan
PRINT
  1 + 2
```

Both print `3`. The indentation rules are strict and small:

- Indent in multiples of two spaces. Odd indentation is rejected.
- Tabs are not allowed.
- Blank lines are ignored.

This is the idiomatic way to lay out anything beyond a trivial expression:
rather than piling parentheses onto one line, break a complex expression across
lines and let the indentation group it. Throughout the tutorial you'll see a
function body, a `MATCH`, or a long argument written indented under the line it
belongs to. Indentation groups *one* nested expression — a *sequence* of
separate statements (a module body with several members, a multi-step function
body) is written as parenthesized groups instead, a form those chapters show.

### When nested expressions run

A nested `(...)` whose result feeds a *value* position runs **eagerly** —
before its parent, so the parent sees the finished value. That is the common
case and the one above. A handful of forms instead take a nested expression
as *unevaluated* data and decide for themselves when (or whether) to run it —
the body of a function, the branches of a match, and quoted expressions. Those
are introduced in their own chapters; until then, every `(...)` you write runs
eagerly.

## A note on comments

Koan has no comment syntax yet. The `#` character is reserved for
[quoting expressions](10-quoting.md), not for comments, so a stray `#` in your
source is a parse error rather than an ignored line. In this tutorial, any
explanation of a snippet lives in the prose around it, and expected output is
shown in a separate block underneath.

Next: [Values and types](02-values-and-types.md).

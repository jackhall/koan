# Koan Tutorial

A walk-through of what you can write in Koan today. Koan is an early prototype, so the surface is small: three real builtins, a handful of literal kinds, and parentheses-or-indentation grouping. This doc covers everything that exists, in the order you would learn it.

If you want the architectural picture (parse → dispatch → execute), see [README.md](README.md). This file is just the user-facing language.

## Running a program

The CLI in [src/main.rs](src/main.rs) reads source from a file argument or from stdin:

```sh
cargo run -- hello.koan
echo 'PRINT "hello"' | cargo run
```

Save snippets below into a `.koan` file, or pipe them in directly.

## Literals

Four kinds, all recognized by [tokens.rs](src/parse/tokens.rs):

```
42            # number (f64 under the hood)
3.14          # number
"hello"       # string (double quotes)
'hello'       # string (single quotes — equivalent)
true          # boolean
false         # boolean
null          # the null value
```

A literal on its own is a complete top-level expression. It dispatches through `value_pass` ([src/dispatch/builtins/value_pass.rs](src/dispatch/builtins/value_pass.rs)) and produces the value, but with nothing to do with the result it is effectively a no-op.

## `LET` — bind a name

```
LET x = 42
LET greeting = "hello"
LET done = true
```

Reads as `LET <name> = <value>`. The right-hand side accepts any scalar literal (number, string, bool, null). After the binding runs, `x`, `greeting`, and `done` are in scope for any later expression in the same program.

You cannot yet `LET` to the result of a non-trivial expression on the right; the binding's value slot is filled at parse time and only scalars round-trip cleanly. See [let_binding.rs](src/dispatch/builtins/let_binding.rs).

## `PRINT` — write a string

```
PRINT "hello, world"
```

Reads as `PRINT <msg>`, where `<msg>` must be a string. It writes the string and a trailing newline to the scope's output sink (stdout by default).

`PRINT` only accepts strings — passing a number is a type mismatch and surfaces as a structured `KError` at the CLI (see "Errors" below).

## Looking up bound names

Once `LET` has bound a name, you can refer back to it:

```
LET msg = "hi"
PRINT msg
```

A bare identifier dispatches through `value_lookup` ([value_lookup.rs](src/dispatch/builtins/value_lookup.rs)), which walks the scope chain to find the bound value. Unbound names produce a structured `unbound name` error (see "Errors" below).

## Sub-expressions with parentheses

Parens group a sequence of tokens into a nested expression that gets dispatched on its own, and its result gets spliced into the enclosing expression.

```
PRINT (LET msg = "hello world!")
```

Here `LET` runs first, returns the bound value (`"hello world!"`), and that value becomes the `msg` argument to `PRINT`. So this both binds `msg` and prints it in one line.

You can nest arbitrarily:

```
LET outer = "x"
PRINT (outer)
```

The inner `(outer)` is a one-token expression that dispatches to `value_lookup`, returning the string bound to `outer`, which `PRINT` then writes.

## `IF ... THEN` — the lazy conditional

```
IF true THEN (PRINT "ran")
IF false THEN (PRINT "skipped")
```

Reads as `IF <predicate> THEN <expression>`. The predicate is eagerly evaluated; the THEN-branch is **only executed when the predicate is true**. This is the one place in the language today where evaluation is lazy.

Important: the THEN-branch must be wrapped in parens. Without parens the rest of the line is parsed as a bare token sequence, not as a sub-expression, and the laziness machinery in [interpret.rs](src/execute/interpret.rs) won't kick in.

```
LET name = "Ada"
IF true THEN (PRINT name)
```

Combine with `PRINT` to get conditional values:

```
PRINT (IF true THEN ("yes"))
```

Here the inner `("yes")` is the lazy branch. Because the predicate is true, it dispatches and produces the string `"yes"`, which `PRINT` then writes. If the predicate were false, the whole `IF ... THEN ...` would return `null` and `PRINT` would receive a non-string and silently drop it.

The "post-hoc selector" caveat in the README applies elsewhere: when an expression is *not* in `IF ... THEN` shape, every nested `(...)` is evaluated eagerly before its parent dispatches. `IF`/`THEN` is special-cased.

## Indentation as block structure

Two-space indents under a parent line nest inside it, the same as wrapping in parens. These two programs are equivalent:

```
PRINT (LET msg = "hi")
```

```
PRINT
  LET msg = "hi"
```

The whitespace pass in [whitespace.rs](src/parse/whitespace.rs) rewrites the indented form into the parenthesized form before any further parsing happens. Rules:

- Tabs are rejected outright.
- Indents must be in multiples of two spaces; an odd number is rejected.
- Blank lines are ignored.

## Putting it together

A small program that exercises everything:

```
LET who = "world"
LET shout = true

IF shout THEN (PRINT who)
```

What happens, in order:

1. `LET who = "world"` binds `who` to the string `"world"`.
2. `LET shout = true` binds `shout` to `true`.
3. `IF shout THEN (PRINT who)` evaluates the predicate (`shout` looks up to `true`), so the lazy branch fires.
4. The lazy branch is `(PRINT who)`. The inner `who` looks up to `"world"`, and `PRINT` writes `world\n` to stdout.

## Errors

Failures surface as structured `KError` values at the CLI rather than silent `null`s. The error prints to stderr with the structured kind followed by a frame chain showing where it came from. Examples:

```
$ echo 'foo' | cargo run
error: unbound name 'foo'

$ echo 'IF "x" THEN ("y")' | cargo run
error: dispatch failed for IF x THEN y: no matching function

$ printf 'FN (BAD) = (undefined)\nBAD\n' | cargo run
error: unbound name 'undefined'
  in fn(BAD) (fn(BAD))
```

There is no in-language try/catch construct — errors propagate to the top level automatically. A future builtin will let in-language code observe and handle errors as values; for now they short-circuit the program. Intentional `null` results (e.g., `IF false THEN x`, `PRINT`'s return value) stay as `null` and do not error.

## What's not in the language yet

Things you might expect that don't exist today — all tracked in [ROADMAP.md](ROADMAP.md):

- **No user-defined types.** `KType` is a closed enum of seven host-defined kinds; you can't declare a record, a variant, or a trait.
- **No arithmetic, comparison, or logical operators.** `1 + 1` does not parse as addition. The token-level operator table in [operators.rs](src/parse/operators.rs) only has compound-token desugarings (`!`, `.`, `[]`, `?`), and those are not wired to runtime behavior.
- **No loops.** Recursion is the iteration model now that user functions exist (see [ROADMAP.md](ROADMAP.md)'s leak-fix and TCO sections).
- **No in-language error catching.** Errors surface to the CLI but no surface syntax or builtin lets a Koan program inspect and handle them yet.

If a snippet doesn't behave the way you expect, the most likely cause is one of the above, not a bug in your code.
